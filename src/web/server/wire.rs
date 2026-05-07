//! Request parsing and response serialization. The wire layer has no
//! knowledge of routing, auth, or static-asset semantics beyond cache
//! headers — it just turns bytes into [`ParsedRequest`] and a
//! [`Response`] back into bytes.

use std::sync::Arc;

use dashmap::DashMap;
use once_cell::sync::Lazy;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::tcp::OwnedWriteHalf;

#[derive(Debug)]
pub(super) enum ReadError {
    /// Underlying socket error. We do not track the inner error because
    /// the only action on Io is "close the connection" — same as Idle.
    #[allow(dead_code)]
    Io(std::io::Error),
    Idle,
    TooLarge,
}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        ReadError::Io(e)
    }
}

/// Index of the byte immediately after the first `\r\n\r\n` sequence,
/// or `None` if the buffer does not yet contain the terminator.
pub(super) fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

#[derive(Debug)]
pub(super) struct ParsedRequest<'a> {
    pub(super) method: &'a str,
    pub(super) path: &'a str,
    pub(super) authorization: Option<&'a str>,
    pub(super) accepts_gzip: bool,
    /// True when the request advertises `Accept: application/json`. The SPA
    /// `fetch()` wrapper sets this on every call; a browser hitting the URL
    /// directly would not. The mux uses it to skip the `WWW-Authenticate`
    /// header on 401 — otherwise the browser caches whatever the user typed
    /// in its native basic-auth dialog and replays it forever, hiding our
    /// React sign-in modal.
    pub(super) accepts_json: bool,
    /// True when the request explicitly opts out of HTTP/1.1 keep-alive
    /// (`Connection: close`) or speaks an older HTTP version. The mux
    /// uses it to decide whether to drop the connection after the
    /// response or wait for another request on the same socket.
    pub(super) connection_close: bool,
}

impl<'a> ParsedRequest<'a> {
    pub(super) fn parse(raw: &'a str) -> Option<Self> {
        let mut lines = raw.split("\r\n");
        let request_line = lines.next()?;
        let mut parts = request_line.splitn(3, ' ');
        let method = parts.next()?;
        let path = parts.next()?;
        let http_version = parts.next()?;

        let mut authorization = None;
        let mut accepts_gzip = false;
        let mut accepts_json = false;
        let mut connection_close = !http_version.eq_ignore_ascii_case("HTTP/1.1");
        for line in lines {
            if line.is_empty() {
                break;
            }
            // Headers are case-insensitive per RFC 7230. Match by case-
            // insensitive prefix without allocating a lowercase copy of
            // the header value — `to_lowercase()` per request line was
            // codex perf P3#9.
            if let Some(value) = strip_header_prefix(line, "Authorization") {
                authorization = Some(value);
            } else if let Some(value) = strip_header_prefix(line, "Accept-Encoding") {
                if contains_ascii_ci(value, "gzip") {
                    accepts_gzip = true;
                }
            } else if let Some(value) = strip_header_prefix(line, "Accept") {
                if contains_ascii_ci(value, "application/json") {
                    accepts_json = true;
                }
            } else if let Some(value) = strip_header_prefix(line, "Connection") {
                if contains_ascii_ci(value, "close") {
                    connection_close = true;
                }
            }
        }
        Some(ParsedRequest {
            method,
            path,
            authorization,
            accepts_gzip,
            accepts_json,
            connection_close,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) reason: &'static str,
    pub(crate) extra_headers: Vec<(&'static str, String)>,
    pub(crate) body: Vec<u8>,
}

impl Response {
    pub(crate) fn status(status: u16, reason: &'static str) -> Self {
        Response {
            status,
            reason,
            extra_headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub(crate) fn json(status: u16, reason: &'static str, body: &str) -> Self {
        Response {
            status,
            reason,
            extra_headers: vec![("Content-Type", "application/json".into())],
            body: body.as_bytes().to_vec(),
        }
    }

    /// 401 with `WWW-Authenticate`. Use only for non-JSON callers (curl,
    /// direct browser navigation) — the SPA path uses `unauthorized_silent`
    /// to keep the browser from caching credentials we did not solicit.
    pub(crate) fn unauthorized() -> Self {
        let mut r = Response::status(401, "Unauthorized");
        r.extra_headers.push((
            "WWW-Authenticate",
            "Basic realm=\"pg_doorman admin\"".into(),
        ));
        r
    }

    /// 401 without `WWW-Authenticate`. Use for SPA / JSON callers so the
    /// browser does not cache rejected credentials and replay them under
    /// our React modal.
    pub(crate) fn unauthorized_silent() -> Self {
        Response::status(401, "Unauthorized")
    }

    /// Serves a static asset (SPA bundle file). Hashed assets get a long
    /// immutable cache; the SPA shell (`index.html`) is no-cache so a redeploy
    /// reaches operators on their next reload. When the caller advertises
    /// `Accept-Encoding: gzip` and the asset compresses worthwhile (text-like
    /// MIME, > 256 bytes), the body is gzipped on the fly — that turns the
    /// ~280 KB JS bundle into ~95 KB on the wire.
    pub(crate) fn static_asset(
        asset: &crate::web::static_assets::Asset,
        accepts_gzip: bool,
    ) -> Self {
        let cache = if asset.immutable {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        let mut headers = vec![
            ("Content-Type", asset.mime.into()),
            ("Cache-Control", cache.into()),
        ];
        let body = if accepts_gzip && is_compressible(asset.mime) && asset.bytes.len() > 256 {
            match gzip_cached(asset) {
                Some(bytes) => {
                    headers.push(("Content-Encoding", "gzip".into()));
                    bytes
                }
                None => asset.bytes.to_vec(),
            }
        } else {
            asset.bytes.to_vec()
        };
        Response {
            status: 200,
            reason: "OK",
            extra_headers: headers,
            body,
        }
    }

    /// Override the status line on a Response built via [`Response::ok_json`].
    /// Useful when the body shape is the same JSON envelope but the
    /// outcome should travel back as 4xx/5xx — codex Arch P2#4 admin
    /// route refactor.
    pub(crate) fn with_status(mut self, status: u16, reason: &'static str) -> Self {
        self.status = status;
        self.reason = reason;
        self
    }

    pub(crate) fn ok_json<T: serde::Serialize>(value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => Response {
                status: 200,
                reason: "OK",
                extra_headers: vec![("Content-Type", "application/json".into())],
                body,
            },
            Err(e) => {
                log::error!("Failed to serialize JSON response: {e}");
                Response::status(500, "Internal Server Error")
            }
        }
    }

    pub(super) async fn write(self, writer: &mut BufWriter<OwnedWriteHalf>) -> std::io::Result<()> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n",
            self.status,
            self.reason,
            self.body.len()
        );
        for (k, v) in &self.extra_headers {
            head.push_str(k);
            head.push_str(": ");
            head.push_str(v);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        writer.write_all(head.as_bytes()).await?;
        if !self.body.is_empty() {
            writer.write_all(&self.body).await?;
        }
        writer.flush().await
    }
}

pub(super) async fn write_simple(
    writer: &mut BufWriter<OwnedWriteHalf>,
    status: u16,
    reason: &'static str,
) -> std::io::Result<()> {
    Response::status(status, reason).write(writer).await
}

/// Strip a case-insensitive `Header: ` prefix (header name + `: `)
/// without allocating. Returns the header value when the prefix matches,
/// `None` otherwise. ASCII-only by design — HTTP header names are
/// strictly ASCII per RFC 7230.
fn strip_header_prefix<'a>(line: &'a str, header: &str) -> Option<&'a str> {
    let need = header.len() + 2; // ": "
    let bytes = line.as_bytes();
    if bytes.len() < need {
        return None;
    }
    if !line.as_bytes()[..header.len()].eq_ignore_ascii_case(header.as_bytes()) {
        return None;
    }
    if &bytes[header.len()..need] != b": " {
        return None;
    }
    Some(&line[need..])
}

/// Case-insensitive `contains` over ASCII bytes. Avoids the
/// `value.to_lowercase()` allocation that codex P3#9 flagged.
fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() {
        return true;
    }
    if h.len() < n.len() {
        return false;
    }
    h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// Cache of gzipped static asset bodies, keyed by the asset's path. The
/// SPA bundle is immutable per release, so compressing the JS/CSS once
/// and serving the cached `Vec<u8>` on subsequent requests turns each
/// poll's static-asset hit from "allocate + zlib + compress" into a
/// single Arc clone.
static GZIP_CACHE: Lazy<DashMap<&'static str, Arc<Vec<u8>>>> = Lazy::new(DashMap::new);

fn gzip_cached(asset: &crate::web::static_assets::Asset) -> Option<Vec<u8>> {
    if let Some(entry) = GZIP_CACHE.get(asset.path) {
        return Some(entry.value().as_ref().clone());
    }
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut compressed = Vec::with_capacity(asset.bytes.len() / 2);
    {
        let mut gz = GzEncoder::new(&mut compressed, Compression::default());
        gz.write_all(asset.bytes).ok()?;
        gz.finish().ok()?;
    }
    let arc = Arc::new(compressed);
    GZIP_CACHE.insert(asset.path, arc.clone());
    Some(arc.as_ref().clone())
}

/// Compressing a 200-byte favicon PNG buys nothing and risks negative
/// ratios; only compress text-like payloads where gzip pays off.
fn is_compressible(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime.starts_with("application/javascript")
        || mime.starts_with("application/json")
        || mime.starts_with("image/svg+xml")
}
