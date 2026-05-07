//! HTTP/1.1 keep-alive connection driver. Each accepted socket runs through
//! [`handle_connection`], which loops reading request heads off a per-connection
//! buffer, dispatches to the router (or to async log/admin handlers when the
//! response cannot be produced synchronously), and stops when the client
//! signals close or the per-connection request cap is hit.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, BufReader, BufWriter};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::TcpStream;

use crate::web::auth::{classify, AuthOutcome, Role};
use crate::web::metrics::write_metrics_response;

use super::router::{dispatch, unauthorized_for};
use super::state::WebServerOptions;
use super::wire::{find_double_crlf, write_simple, ParsedRequest, ReadError, Response};

/// Soft cap on requests per keep-alive connection. After this many
/// requests we close so a misbehaving client cannot pin a worker
/// forever; HTTP/1.1 clients that need more will reconnect.
const KEEPALIVE_MAX_REQUESTS: u32 = 1000;

/// Idle timeout between requests on a keep-alive connection. Browsers
/// hold these open for minutes by default; pg_doorman terminates faster
/// because each idle connection still costs an FD and a tokio task.
const KEEPALIVE_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) async fn handle_connection(stream: TcpStream, opts: Arc<WebServerOptions>) {
    let peer_addr = stream.peer_addr().ok();
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut writer = BufWriter::new(write_half);

    let mut req_buf: Vec<u8> = Vec::with_capacity(4096);
    let mut handled = 0u32;
    while handled < KEEPALIVE_MAX_REQUESTS {
        // `req_buf` carries over any bytes from the previous read that
        // belonged to the *next* request (clients can pipeline two GETs
        // into one TCP write). `read_request_head` extends it until the
        // header terminator is in view, then we slice off only the
        // first request and keep the tail for the next iteration.
        let head_end = match read_request_head(&mut reader, &mut req_buf).await {
            Ok(0) => return, // peer closed cleanly between requests
            Ok(end) => end,
            Err(ReadError::Io(_)) | Err(ReadError::Idle) => return,
            Err(ReadError::TooLarge) => {
                let _ = write_simple(&mut writer, 431, "Request Header Fields Too Large").await;
                return;
            }
        };
        let started = std::time::Instant::now();
        let head_bytes = &req_buf[..head_end];
        let raw = match std::str::from_utf8(head_bytes) {
            Ok(s) => s,
            Err(_) => {
                let _ = write_simple(&mut writer, 400, "Bad Request").await;
                return;
            }
        };
        let Some(parsed) = ParsedRequest::parse(raw) else {
            let _ = write_simple(&mut writer, 400, "Bad Request").await;
            return;
        };
        let close_after = parsed.connection_close;

        // Pre-compute the access-log fields we need from `parsed` before
        // it goes out of scope.
        let log_method = parsed.method.to_string();
        let log_path = parsed.path.to_string();
        let log_query_present = parsed.query.is_some();

        // /metrics is always served, regardless of ui_active or auth.
        // It writes its body directly through the gzip-aware response
        // writer, so we don't build a Response struct here.
        if parsed.method == "GET" && parsed.path == "/metrics" {
            write_metrics_response(&mut writer, parsed.accepts_gzip).await;
            crate::web::access_log::write(
                &log_method,
                &log_path,
                log_query_present,
                200,
                0,
                started.elapsed().as_millis() as u64,
                peer_addr,
                &AuthOutcome::Anonymous,
            );
            req_buf.drain(..head_end);
            handled += 1;
            if close_after {
                return;
            }
            continue;
        }

        let auth = classify(
            parsed.authorization,
            parsed.cookie,
            extract_query_token(parsed.query),
            &opts.admin_username,
            &opts.admin_password,
            opts.sso.as_deref(),
        );

        // /api/logs needs an async handler because it talks to the LogTap
        // consumer task via mpsc + oneshot; the rest of the API stays sync.
        // Pre-screen ui_active and the role here so dispatch() never sees
        // the path on the success branch — on failure we fall through to
        // dispatch() which already returns the right 401/404.
        let response = if opts.ui_active && parsed.method == "GET" && parsed.path == "/api/logs" {
            if matches!(auth, AuthOutcome::Rejected) || auth.role() < Role::Sso {
                unauthorized_for(&parsed)
            } else {
                let query = crate::web::routes::query::parse_query(parsed.query.unwrap_or(""));
                crate::web::routes::logs::handle_logs(&query).await
            }
        } else if opts.ui_active
            && parsed.method == "POST"
            && parsed.path.starts_with("/api/admin/")
        {
            if !matches!(auth, AuthOutcome::Admin(_)) {
                if matches!(auth, AuthOutcome::Sso(_)) {
                    Response::forbidden("admin role required")
                } else {
                    unauthorized_for(&parsed)
                }
            } else {
                crate::web::routes::admin::handle_admin_action(parsed.path).await
            }
        } else {
            dispatch(&parsed, &opts, &auth)
        };

        let status = response.status;
        let bytes = response.body.len();
        let _ = response.write(&mut writer).await;
        crate::web::access_log::write(
            &log_method,
            &log_path,
            log_query_present,
            status,
            bytes,
            started.elapsed().as_millis() as u64,
            peer_addr,
            &auth,
        );

        // Discard the request we just answered; pipelined bytes (a
        // second request that came in the same TCP read) stay at the
        // head of `req_buf` for the next iteration to consume.
        req_buf.drain(..head_end);
        handled += 1;
        if close_after {
            return;
        }
    }
    // Hit the per-connection request cap. Close so the client knows to
    // reconnect rather than queue more behind us.
}

/// Pick `token=<jwt>` out of a raw query string, returning the token
/// substring without URL-decoding. JWTs are base64url so they round-trip
/// through query strings unchanged; if the proxy URL-encoded the token
/// (replacing `+/=`), `SsoRuntime::validate` rejects it and the SPA
/// retries via Bearer header.
fn extract_query_token(query: Option<&str>) -> Option<&str> {
    let q = query?;
    q.split('&').find_map(|pair| pair.strip_prefix("token="))
}

/// Extend `buf` with bytes from the wire until the request-header
/// terminator `\r\n\r\n` is in view. Returns the offset *just past* the
/// terminator (so the caller knows where the headers end and any
/// pipelined body / next request begin), or `Ok(0)` if the peer closed
/// cleanly between requests. Caps the buffer at 32 KiB so a malicious
/// client cannot push us into OOM.
async fn read_request_head(
    reader: &mut BufReader<OwnedReadHalf>,
    buf: &mut Vec<u8>,
) -> Result<usize, ReadError> {
    const MAX_HEADER_BYTES: usize = 32 * 1024;
    if buf.is_empty() {
        // Wait up to KEEPALIVE_IDLE_TIMEOUT for the first byte; once
        // bytes arrive, the read loop below drives without an outer
        // timeout because the headers are bounded by MAX_HEADER_BYTES.
        let mut chunk = [0u8; 1024];
        let read_fut = reader.read(&mut chunk);
        let n = match tokio::time::timeout(KEEPALIVE_IDLE_TIMEOUT, read_fut).await {
            Ok(r) => r?,
            Err(_elapsed) => return Err(ReadError::Idle),
        };
        if n == 0 {
            return Ok(0);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    if let Some(end) = find_double_crlf(buf) {
        return Ok(end);
    }
    let mut chunk = [0u8; 1024];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            // Peer closed mid-request — treat as malformed.
            return Err(ReadError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "EOF mid request headers",
            )));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = find_double_crlf(buf) {
            return Ok(end);
        }
        if buf.len() >= MAX_HEADER_BYTES {
            return Err(ReadError::TooLarge);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_query_token_returns_value_when_only_param() {
        assert_eq!(
            extract_query_token(Some("token=abc.def.ghi")),
            Some("abc.def.ghi")
        );
    }

    #[test]
    fn extract_query_token_returns_value_when_among_others() {
        assert_eq!(
            extract_query_token(Some("foo=1&token=jwt&bar=2")),
            Some("jwt")
        );
    }

    #[test]
    fn extract_query_token_handles_trailing_amp() {
        assert_eq!(extract_query_token(Some("token=jwt&")), Some("jwt"));
    }

    #[test]
    fn extract_query_token_returns_first_match() {
        // Two `token=` keys would be malformed but the function must be
        // deterministic — the first wins.
        assert_eq!(
            extract_query_token(Some("token=first&token=second")),
            Some("first")
        );
    }

    #[test]
    fn extract_query_token_rejects_keys_with_token_as_substring() {
        // `mytoken=foo` must NOT match — `strip_prefix("token=")` only
        // matches at the start of a pair.
        assert_eq!(extract_query_token(Some("mytoken=foo&other=bar")), None);
    }

    #[test]
    fn extract_query_token_returns_empty_for_token_without_value() {
        assert_eq!(extract_query_token(Some("token=")), Some(""));
    }

    #[test]
    fn extract_query_token_returns_none_for_no_token_key() {
        assert_eq!(extract_query_token(Some("foo=1&bar=2")), None);
    }

    #[test]
    fn extract_query_token_returns_none_for_empty_query() {
        assert_eq!(extract_query_token(Some("")), None);
    }

    #[test]
    fn extract_query_token_returns_none_for_none_input() {
        assert_eq!(extract_query_token(None), None);
    }
}
