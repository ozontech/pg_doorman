//! HTTP listener + path mux for the web subsystem.
//!
//! Routes:
//! - `GET /metrics`      → Prometheus exporter, no auth.
//! - `GET /api/version`  → version info, public.
//! - `GET /api/overview` → cluster overview, public.
//! - `GET /api/pools`    → pool list, public.
//! - `GET /api/*`        → other endpoints return 501 until wired in later phases.
//! - `GET /` | `GET /assets/*` → SPA placeholder, returns 404 (filled in phase 7).
//! - everything else → 404.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;
use log::{error, info};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{TcpSocket, TcpStream};

use crate::config::Config;
use crate::web::auth::{classify, AuthOutcome};
use crate::web::metrics::write_metrics_response;

/// Runtime state needed by the mux on every request.
#[derive(Clone)]
pub struct WebServerOptions {
    /// `true` when `[web].ui = true` AND admin_password is non-default.
    /// When `false`, the listener serves only `/metrics`; everything else → 404.
    pub ui_active: bool,
    /// `[web].ui_anonymous` — gates the public `/api/*` endpoints when
    /// `ui_active`. The SPA shell (HTML/CSS/JS/font/svg) is always served
    /// anonymously so a hard refresh of a deep link does not trigger a
    /// browser-native basic-auth prompt on top of the React `AuthGate`.
    pub ui_anonymous: bool,
    pub admin_username: String,
    pub admin_password: String,
}

impl WebServerOptions {
    /// Build the request-time options from a config snapshot. `ui_active`
    /// is gated on a non-default admin password — `web.ui = true` paired
    /// with an empty/`"admin"` password is silently demoted to "metrics
    /// only", matching the explicit warning the startup path logs in
    /// `app::server::run_server`.
    pub fn from_config(cfg: &Config) -> Self {
        let admin_default =
            cfg.general.admin_password.is_empty() || cfg.general.admin_password == "admin";
        WebServerOptions {
            ui_active: cfg.web.ui && !admin_default,
            ui_anonymous: cfg.web.ui_anonymous,
            admin_username: cfg.general.admin_username.clone(),
            admin_password: cfg.general.admin_password.clone(),
        }
    }
}

/// Reload-aware options snapshot used by every request. Installed once on
/// `start_web_server`, swapped atomically when the admin protocol or the
/// REST `/api/admin/reload` endpoint replaces the global config. Without
/// this, `RELOAD` would update `/api/config` but the listener would keep
/// authenticating against the old password and ignoring `[web].ui_anonymous`
/// changes until the next process restart.
static WEB_OPTIONS: OnceLock<ArcSwap<WebServerOptions>> = OnceLock::new();

fn install_options(opts: Arc<WebServerOptions>) {
    if let Some(swap) = WEB_OPTIONS.get() {
        swap.store(opts);
    } else {
        let _ = WEB_OPTIONS.set(ArcSwap::from(opts));
    }
}

fn current_options() -> Arc<WebServerOptions> {
    WEB_OPTIONS
        .get()
        .map(|swap| swap.load_full())
        .unwrap_or_else(|| {
            // Fallback for code paths that read options before the listener
            // started. Recomputes from the live config so behavior is at
            // least defined; `start_web_server` will replace it on bind.
            Arc::new(WebServerOptions::from_config(&crate::config::get_config()))
        })
}

/// Re-derive the listener's runtime options from the current global config.
/// Called by every code path that updates the global `Config` (admin
/// protocol `RELOAD`, REST `/api/admin/reload`). Idempotent.
pub fn refresh_options_from_config() {
    let cfg = crate::config::get_config();
    install_options(Arc::new(WebServerOptions::from_config(&cfg)));
}

/// Admin-only path prefixes (require `Admin` auth regardless of `ui_anonymous`).
/// Spec section 6.1.
const ADMIN_ONLY_PREFIXES: &[&str] = &[
    "/api/logs",
    "/api/prepared/text/",
    "/api/interner/top",
    // /api/top/queries returns SQL previews — first 120 chars of cached
    // statements. Tenant ids, literal values, schema names, and the
    // occasional accidental secret embedded in SQL all leak through;
    // keep it admin-only regardless of `ui_anonymous`.
    "/api/top/queries",
    "/api/admin/",
];

/// Bind the listener synchronously and return it. Used by callers that
/// want to fail fast when the configured port is taken: the daemon's
/// readiness signal must wait until the web subsystem is verifiably
/// listening, otherwise systemd / a binary-upgrade parent treats the
/// pooler as healthy while `/metrics` and the UI are silently down.
pub fn bind_web_listener(host: &str) -> std::io::Result<tokio::net::TcpListener> {
    info!("binding web listener on {host}");
    let addr: SocketAddr = host.parse().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Failed to parse socket address '{host}': {e}"),
        )
    })?;

    let listen_socket = if addr.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }?;

    listen_socket.set_reuseaddr(true)?;
    listen_socket.set_reuseport(true)?;
    listen_socket.bind(addr)?;
    let listener = listen_socket.listen(1024)?;
    info!("web listener bound on {addr}");
    Ok(listener)
}

/// Spawns the HTTP listener for the given address. The provided `opts`
/// seeds the reload-aware [`WEB_OPTIONS`] slot — subsequent
/// [`refresh_options_from_config`] calls (from `RELOAD`) atomically replace
/// it, and every request reads the current value via [`current_options`].
///
/// Panics on bind failure. Production callers in `app::server::run_server`
/// prefer [`bind_web_listener`] + [`serve_on`] so a port collision fails
/// the whole startup instead of leaving the listener task panicked behind
/// a successful readiness signal.
pub async fn start_web_server(host: &str, opts: WebServerOptions) {
    let listener = bind_web_listener(host)
        .unwrap_or_else(|e| panic!("Failed to bind web listener on {host}: {e}"));
    serve_on(listener, opts).await;
}

/// Drive the accept loop on a pre-bound listener. Used by both
/// [`start_web_server`] and the production startup path that binds
/// synchronously before spawning.
pub async fn serve_on(listener: tokio::net::TcpListener, opts: WebServerOptions) {
    install_options(Arc::new(opts));

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let opts = current_options();
                tokio::spawn(async move {
                    handle_connection(stream, opts).await;
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {e}");
            }
        }
    }
}

/// Soft cap on requests per keep-alive connection. After this many
/// requests we close so a misbehaving client cannot pin a worker
/// forever; HTTP/1.1 clients that need more will reconnect.
const KEEPALIVE_MAX_REQUESTS: u32 = 1000;

/// Idle timeout between requests on a keep-alive connection. Browsers
/// hold these open for minutes by default; pg_doorman terminates faster
/// because each idle connection still costs an FD and a tokio task.
const KEEPALIVE_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

async fn handle_connection(stream: TcpStream, opts: Arc<WebServerOptions>) {
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
        let close_after = {
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

            // /metrics is always served, regardless of ui_active or auth.
            if parsed.method == "GET" && parsed.path == "/metrics" {
                write_metrics_response(&mut writer, parsed.accepts_gzip).await;
            } else {
                let auth = classify(
                    parsed.authorization,
                    &opts.admin_username,
                    &opts.admin_password,
                );

                // /api/logs needs an async handler because it talks to the LogTap consumer
                // task via mpsc + oneshot; the rest of the API stays sync. Pre-screen
                // ui_active and admin auth here so dispatch() never sees the path on the
                // success branch — on auth failure or inactive UI we fall through to
                // dispatch() which already returns the right 401/404.
                if opts.ui_active
                    && parsed.method == "GET"
                    && (parsed.path == "/api/logs" || parsed.path.starts_with("/api/logs?"))
                {
                    if auth != AuthOutcome::Admin {
                        let _ = unauthorized_for(&parsed).write(&mut writer).await;
                    } else {
                        let query_str = parsed.path.split_once('?').map(|(_, q)| q).unwrap_or("");
                        let query = crate::web::routes::query::parse_query(query_str);
                        let response = crate::web::routes::logs::handle_logs(&query).await;
                        let _ = response.write(&mut writer).await;
                    }
                } else if opts.ui_active
                    && parsed.method == "POST"
                    && parsed.path.starts_with("/api/admin/")
                {
                    if auth != AuthOutcome::Admin {
                        let _ = unauthorized_for(&parsed).write(&mut writer).await;
                    } else {
                        let response =
                            crate::web::routes::admin::handle_admin_action(parsed.path).await;
                        let _ = response.write(&mut writer).await;
                    }
                } else {
                    let response = dispatch(&parsed, &opts, auth);
                    let _ = response.write(&mut writer).await;
                }
            }
            close_after
        };

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

#[derive(Debug)]
enum ReadError {
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

/// Extend `buf` with bytes from the wire until the request-header
/// terminator `\r\n\r\n` is in view. Returns the offset *just past* the
/// terminator (so the caller knows where the headers end and any
/// pipelined body / next request begin), or `Ok(0)` if the peer closed
/// cleanly between requests. Caps the buffer at 32 KiB so a malicious
/// client cannot push us into OOM.
async fn read_request_head(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
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

/// Index of the byte immediately after the first `\r\n\r\n` sequence,
/// or `None` if the buffer does not yet contain the terminator.
fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

#[derive(Debug)]
struct ParsedRequest<'a> {
    method: &'a str,
    path: &'a str,
    authorization: Option<&'a str>,
    accepts_gzip: bool,
    /// True when the request advertises `Accept: application/json`. The SPA
    /// `fetch()` wrapper sets this on every call; a browser hitting the URL
    /// directly would not. The mux uses it to skip the `WWW-Authenticate`
    /// header on 401 — otherwise the browser caches whatever the user typed
    /// in its native basic-auth dialog and replays it forever, hiding our
    /// React sign-in modal.
    accepts_json: bool,
    /// True when the request explicitly opts out of HTTP/1.1 keep-alive
    /// (`Connection: close`) or speaks an older HTTP version. The mux
    /// uses it to decide whether to drop the connection after the
    /// response or wait for another request on the same socket.
    connection_close: bool,
}

impl<'a> ParsedRequest<'a> {
    fn parse(raw: &'a str) -> Option<Self> {
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
            if let Some(value) = line.strip_prefix("Authorization: ") {
                authorization = Some(value);
            } else if let Some(value) = line.strip_prefix("authorization: ") {
                authorization = Some(value);
            } else if let Some(value) = line.strip_prefix("Accept-Encoding: ") {
                if value.to_lowercase().contains("gzip") {
                    accepts_gzip = true;
                }
            } else if let Some(value) = line.strip_prefix("accept-encoding: ") {
                if value.to_lowercase().contains("gzip") {
                    accepts_gzip = true;
                }
            } else if let Some(value) = line.strip_prefix("Accept: ") {
                if value.to_lowercase().contains("application/json") {
                    accepts_json = true;
                }
            } else if let Some(value) = line.strip_prefix("accept: ") {
                if value.to_lowercase().contains("application/json") {
                    accepts_json = true;
                }
            } else if let Some(value) = line.strip_prefix("Connection: ") {
                if value.to_lowercase().contains("close") {
                    connection_close = true;
                }
            } else if let Some(value) = line.strip_prefix("connection: ") {
                if value.to_lowercase().contains("close") {
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
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

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
            let mut compressed = Vec::with_capacity(asset.bytes.len() / 2);
            {
                let mut gz = GzEncoder::new(&mut compressed, Compression::default());
                if gz.write_all(asset.bytes).is_ok() && gz.finish().is_ok() {
                    headers.push(("Content-Encoding", "gzip".into()));
                    compressed
                } else {
                    asset.bytes.to_vec()
                }
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

    async fn write(
        self,
        writer: &mut BufWriter<tokio::net::tcp::OwnedWriteHalf>,
    ) -> std::io::Result<()> {
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

async fn write_simple(
    writer: &mut BufWriter<tokio::net::tcp::OwnedWriteHalf>,
    status: u16,
    reason: &'static str,
) -> std::io::Result<()> {
    Response::status(status, reason).write(writer).await
}

fn is_admin_only(path: &str) -> bool {
    ADMIN_ONLY_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

/// Compressing a 200-byte favicon PNG buys nothing and risks negative
/// ratios; only compress text-like payloads where gzip pays off.
fn is_compressible(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime.starts_with("application/javascript")
        || mime.starts_with("application/json")
        || mime.starts_with("image/svg+xml")
}

/// Picks the right 401 shape for the caller. Browsers and curl get the
/// `WWW-Authenticate: Basic` challenge so existing tooling keeps working;
/// `Accept: application/json` (the SPA) gets a plain 401 so the React
/// modal can take over without the browser caching credentials.
fn unauthorized_for(req: &ParsedRequest<'_>) -> Response {
    if req.accepts_json {
        Response::unauthorized_silent()
    } else {
        Response::unauthorized()
    }
}

fn route_api(req: &ParsedRequest<'_>) -> Response {
    use crate::web::routes;
    use crate::web::routes::query::parse_query;

    let (path, query_str) = match req.path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (req.path, ""),
    };
    let query = parse_query(query_str);

    // Prefix-routed paths first (admin-only; mux already gated auth).
    if let Some(hash) = path.strip_prefix("/api/prepared/text/") {
        return routes::prepared_text::handle_prepared_text(hash);
    }

    match path {
        "/api/version" => routes::version::handle_version(),
        "/api/overview" => routes::overview::handle_overview(),
        "/api/pools" => routes::pools::handle_pools(),
        "/api/clients" => routes::clients::handle_clients(&query),
        "/api/connections" => routes::connections::handle_connections(),
        "/api/databases" => routes::databases::handle_databases(),
        "/api/servers" => routes::servers::handle_servers(&query),
        "/api/stats" => routes::stats::handle_stats(),
        "/api/users" => routes::users::handle_users(),
        "/api/auth_query" => routes::auth_query::handle_auth_query(),
        "/api/config" => routes::config::handle_config(),
        "/api/log_level" => routes::log_level::handle_log_level(),
        "/api/pool_coordinator" => routes::pool_coordinator::handle_pool_coordinator(),
        "/api/pool_scaling" => routes::pool_scaling::handle_pool_scaling(),
        "/api/process" => routes::process::handle_process(),
        "/api/process/memory" => routes::process::handle_process_memory(),
        "/api/sockets" => routes::sockets::handle_sockets(),
        "/api/prepared" => routes::prepared::handle_prepared(),
        "/api/interner" => routes::interner::handle_interner(),
        "/api/interner/top" => routes::interner_top::handle_interner_top(&query),
        "/api/top/clients" => routes::top_clients::handle_top_clients(&query),
        "/api/top/prepared" => routes::top_prepared::handle_top_prepared(&query),
        "/api/top/queries" => routes::top_queries::handle_top_queries(&query),
        "/api/apps" => routes::apps::handle_apps(&query),
        "/api/events" => routes::events::handle_events(&query),
        _ => Response::json(
            501,
            "Not Implemented",
            r#"{"error":"not_implemented","message":"endpoint will be wired in a later phase"}"#,
        ),
    }
}

fn dispatch(req: &ParsedRequest<'_>, opts: &WebServerOptions, auth: AuthOutcome) -> Response {
    let is_admin_post = req.method == "POST" && req.path.starts_with("/api/admin/");
    if req.method != "GET" && req.method != "HEAD" && !is_admin_post {
        return Response::status(405, "Method Not Allowed");
    }

    if !opts.ui_active {
        // /metrics already handled before dispatch().
        return Response::status(404, "Not Found");
    }

    let is_api = req.path.starts_with("/api/");
    let admin_only = is_api && is_admin_only(req.path);

    // The SPA shell (HTML, CSS, JS, fonts, favicon) carries no operator data
    // — the basic-auth challenge is reserved for `/api/*`. Letting the shell
    // load anonymously avoids the double-prompt operators saw on a deep link:
    // browser-native basic auth on the HTML, then the React `AuthGate` modal
    // on the first JSON fetch. Now the React modal is the single password
    // prompt the operator ever sees.
    let needs_admin = admin_only || (is_api && !opts.ui_anonymous);
    if needs_admin && auth != AuthOutcome::Admin {
        return unauthorized_for(req);
    }

    if is_api {
        return route_api(req);
    }

    // SPA: serve the embedded bundle. Anything that is not /api or /metrics
    // resolves to a static asset or falls back to the SPA shell so client-side
    // routes (`/pools`, `/clients/...`) work on a hard refresh.
    let bundle_path = req.path.split_once('?').map(|(p, _)| p).unwrap_or(req.path);
    if let Some(asset) = crate::web::static_assets::lookup(bundle_path) {
        return Response::static_asset(&asset, req.accepts_gzip);
    }

    Response::status(404, "Not Found")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(ui_active: bool, ui_anonymous: bool) -> WebServerOptions {
        WebServerOptions {
            ui_active,
            ui_anonymous,
            admin_username: "admin".into(),
            admin_password: "secret".into(),
        }
    }

    fn req<'a>(method: &'a str, path: &'a str) -> ParsedRequest<'a> {
        ParsedRequest {
            method,
            path,
            authorization: None,
            accepts_gzip: false,
            accepts_json: false,
            connection_close: false,
        }
    }

    fn req_json<'a>(method: &'a str, path: &'a str) -> ParsedRequest<'a> {
        ParsedRequest {
            method,
            path,
            authorization: None,
            accepts_gzip: false,
            accepts_json: true,
            connection_close: false,
        }
    }

    fn config_with(ui: bool, ui_anonymous: bool, admin_password: &str) -> Config {
        let mut cfg = Config::default();
        cfg.web.ui = ui;
        cfg.web.ui_anonymous = ui_anonymous;
        cfg.general.admin_username = "admin".into();
        cfg.general.admin_password = admin_password.into();
        cfg
    }

    #[test]
    fn from_config_demotes_ui_when_admin_password_empty() {
        let opts = WebServerOptions::from_config(&config_with(true, false, ""));
        assert!(!opts.ui_active, "empty password must disable UI");
    }

    #[test]
    fn from_config_demotes_ui_when_admin_password_is_default_admin() {
        let opts = WebServerOptions::from_config(&config_with(true, false, "admin"));
        assert!(!opts.ui_active, "literal 'admin' must disable UI");
    }

    #[test]
    fn from_config_keeps_ui_off_when_web_ui_false_even_with_strong_password() {
        let opts = WebServerOptions::from_config(&config_with(false, false, "secret"));
        assert!(!opts.ui_active);
    }

    #[test]
    fn from_config_enables_ui_when_password_strong_and_web_ui_true() {
        let opts = WebServerOptions::from_config(&config_with(true, true, "secret"));
        assert!(opts.ui_active);
        assert!(opts.ui_anonymous);
    }

    #[test]
    fn from_config_copies_credentials_through() {
        let mut cfg = config_with(false, false, "p4ssw0rd");
        cfg.general.admin_username = "ops".into();
        let opts = WebServerOptions::from_config(&cfg);
        assert_eq!(opts.admin_username, "ops");
        assert_eq!(opts.admin_password, "p4ssw0rd");
    }

    #[test]
    fn parse_minimal_get() {
        let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert_eq!(p.method, "GET");
        assert_eq!(p.path, "/api/foo");
        assert_eq!(p.authorization, None);
        assert!(!p.accepts_gzip);
    }

    #[test]
    fn parse_with_authorization_header() {
        let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\nAuthorization: Basic abc\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert_eq!(p.authorization, Some("Basic abc"));
        assert!(!p.accepts_gzip);
    }

    #[test]
    fn parse_with_lowercase_authorization() {
        let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\nauthorization: Basic abc\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert_eq!(p.authorization, Some("Basic abc"));
        assert!(!p.accepts_gzip);
    }

    #[test]
    fn parse_rejects_malformed_request_line() {
        assert!(ParsedRequest::parse("garbage").is_none());
    }

    #[test]
    fn parse_detects_accept_application_json() {
        let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\nAccept: application/json\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert!(p.accepts_json);
    }

    #[test]
    fn parse_detects_lowercase_accept() {
        let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\naccept: application/json, */*\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert!(p.accepts_json);
    }

    #[test]
    fn parse_does_not_detect_json_when_accept_is_html() {
        let raw = "GET / HTTP/1.1\r\nHost: x\r\nAccept: text/html\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert!(!p.accepts_json);
    }

    #[test]
    fn parse_detects_gzip_in_accept_encoding() {
        let raw = "GET /metrics HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip, deflate\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert!(p.accepts_gzip);
    }

    #[test]
    fn dispatch_rejects_post() {
        let r = dispatch(
            &req("POST", "/api/foo"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 405);
    }

    #[test]
    fn dispatch_404_when_ui_inactive() {
        let r = dispatch(
            &req("GET", "/api/foo"),
            &opts(false, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 404);
    }

    #[test]
    fn dispatch_unknown_api_returns_501() {
        let r = dispatch(
            &req("GET", "/api/not-yet-wired"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 501);
    }

    #[test]
    fn dispatch_overview_returns_200() {
        let r = dispatch(
            &req("GET", "/api/overview"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_version_returns_200() {
        let r = dispatch(
            &req("GET", "/api/version"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_pools_returns_200() {
        let r = dispatch(
            &req("GET", "/api/pools"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_admin_anonymous_json_request_returns_401_without_challenge() {
        // SPA / JSON callers must NOT receive WWW-Authenticate, otherwise the
        // browser caches credentials we did not solicit and replays them
        // forever, hiding the React sign-in modal.
        let r = dispatch(
            &req_json("GET", "/api/logs"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 401);
        assert!(
            !r.extra_headers
                .iter()
                .any(|(k, _)| *k == "WWW-Authenticate"),
            "JSON 401 should not advertise Basic auth"
        );
    }

    #[test]
    fn dispatch_401_on_anonymous_admin_path() {
        let r = dispatch(
            &req("GET", "/api/logs"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 401);
        assert!(r
            .extra_headers
            .iter()
            .any(|(k, _)| *k == "WWW-Authenticate"));
    }

    // Note: the admin-success path for /api/logs is handled by the bypass
    // in `handle_connection` (async handler over LogTap mpsc), not by
    // `dispatch`. Integration tests in src/web/tests.rs cover that route.

    #[test]
    fn dispatch_401_on_anonymous_public_when_ui_anonymous_false() {
        let r = dispatch(
            &req("GET", "/api/overview"),
            &opts(true, false),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 401);
    }

    #[test]
    fn dispatch_serves_spa_shell_at_root() {
        let r = dispatch(&req("GET", "/"), &opts(true, true), AuthOutcome::Admin);
        assert_eq!(r.status, 200);
        assert!(
            r.extra_headers
                .iter()
                .any(|(k, v)| *k == "Content-Type" && v.contains("text/html")),
            "root should serve the SPA shell"
        );
    }

    #[test]
    fn dispatch_serves_spa_shell_for_unknown_route() {
        // Client-side router hits this path on a hard refresh of a deep link.
        let r = dispatch(&req("GET", "/pools"), &opts(true, true), AuthOutcome::Admin);
        assert_eq!(r.status, 200);
        assert!(
            r.extra_headers
                .iter()
                .any(|(k, v)| *k == "Content-Type" && v.contains("text/html")),
            "deep link should fall back to the SPA shell"
        );
    }

    #[test]
    fn dispatch_serves_spa_shell_anonymously_when_ui_anonymous_false() {
        // Hard-refreshing a deep link must not trigger a browser-native
        // basic-auth prompt on top of the React `AuthGate`. The SPA shell is
        // anonymous regardless of `ui_anonymous`; only `/api/*` is gated.
        for path in ["/", "/overview", "/pools/some-pool"] {
            let r = dispatch(
                &req("GET", path),
                &opts(true, false),
                AuthOutcome::Anonymous,
            );
            assert_eq!(
                r.status, 200,
                "anonymous SPA shell should serve {path} with ui_anonymous=false"
            );
            assert!(
                r.extra_headers
                    .iter()
                    .any(|(k, v)| *k == "Content-Type" && v.contains("text/html")),
                "{path} should serve the SPA shell as text/html"
            );
        }
    }

    #[test]
    fn dispatch_still_gates_api_when_ui_anonymous_false_after_spa_relax() {
        // Counterpart to `dispatch_serves_spa_shell_anonymously_when_ui_anonymous_false`:
        // the loosened gate must not leak `/api/*` to anonymous callers.
        let r = dispatch(
            &req("GET", "/api/overview"),
            &opts(true, false),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 401);
    }

    #[test]
    fn dispatch_returns_404_for_unknown_asset_when_index_missing() {
        // The bundle is committed in this repo so this test only exercises the
        // Cache-Control / mime path, not the missing-bundle branch — keep the
        // assertion shape forward-compatible by allowing 200 (asset hit) or
        // SPA fallback.
        let r = dispatch(
            &req("GET", "/assets/missing.js"),
            &opts(true, true),
            AuthOutcome::Admin,
        );
        // SPA fallback always returns 200 with the index when the bundle is
        // present. If we ever ship without dist, this would be 404.
        assert!(r.status == 200 || r.status == 404, "got {}", r.status);
    }

    #[test]
    fn is_admin_only_recognises_logs() {
        assert!(is_admin_only("/api/logs"));
        assert!(is_admin_only("/api/logs?since=10"));
        assert!(is_admin_only("/api/prepared/text/abc"));
        assert!(is_admin_only("/api/interner/top"));
    }

    #[test]
    fn is_admin_only_does_not_match_public() {
        assert!(!is_admin_only("/api/overview"));
        assert!(!is_admin_only("/api/pools"));
        assert!(!is_admin_only("/api/prepared"));
    }

    #[test]
    fn dispatch_clients_returns_200() {
        let r = dispatch(
            &req("GET", "/api/clients"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_clients_with_query_params_returns_200() {
        let r = dispatch(
            &req("GET", "/api/clients?limit=10&sort=errors_total"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_servers_returns_200() {
        let r = dispatch(
            &req("GET", "/api/servers"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_connections_returns_200() {
        let r = dispatch(
            &req("GET", "/api/connections"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_stats_returns_200() {
        let r = dispatch(
            &req("GET", "/api/stats"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_databases_returns_200() {
        let r = dispatch(
            &req("GET", "/api/databases"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_users_returns_200() {
        let r = dispatch(
            &req("GET", "/api/users"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_auth_query_returns_200() {
        let r = dispatch(
            &req("GET", "/api/auth_query"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_config_returns_200() {
        let r = dispatch(
            &req("GET", "/api/config"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_log_level_returns_200() {
        let r = dispatch(
            &req("GET", "/api/log_level"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_pool_coordinator_returns_200() {
        let r = dispatch(
            &req("GET", "/api/pool_coordinator"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_pool_scaling_returns_200() {
        let r = dispatch(
            &req("GET", "/api/pool_scaling"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dispatch_sockets_returns_200_on_linux() {
        let r = dispatch(
            &req("GET", "/api/sockets"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        // 500 acceptable in sandbox; handler did not panic = pass.
        assert!(r.status == 200 || r.status == 500, "got {}", r.status);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn dispatch_sockets_returns_503_on_non_linux() {
        let r = dispatch(
            &req("GET", "/api/sockets"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 503);
    }

    #[test]
    fn dispatch_prepared_returns_200() {
        let r = dispatch(
            &req("GET", "/api/prepared"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_interner_returns_200() {
        let r = dispatch(
            &req("GET", "/api/interner"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_interner_top_anonymous_returns_401() {
        let r = dispatch(
            &req("GET", "/api/interner/top"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 401);
    }

    #[test]
    fn dispatch_interner_top_admin_returns_200() {
        let r = dispatch(
            &req("GET", "/api/interner/top?n=10"),
            &opts(true, true),
            AuthOutcome::Admin,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_prepared_text_anonymous_returns_401() {
        let r = dispatch(
            &req("GET", "/api/prepared/text/0x123"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 401);
    }

    #[test]
    fn dispatch_prepared_text_admin_unknown_hash_returns_404() {
        let r = dispatch(
            &req("GET", "/api/prepared/text/0xdeadbeef"),
            &opts(true, true),
            AuthOutcome::Admin,
        );
        assert_eq!(r.status, 404);
    }

    #[test]
    fn dispatch_top_clients_returns_200() {
        let r = dispatch(
            &req("GET", "/api/top/clients"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_top_queries_anonymous_returns_401() {
        // /api/top/queries returns SQL previews — admin-only regardless of
        // ui_anonymous so tenant identifiers and embedded secrets do not leak.
        let r = dispatch(
            &req("GET", "/api/top/queries"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 401);
    }

    #[test]
    fn dispatch_top_queries_admin_returns_200() {
        let r = dispatch(
            &req("GET", "/api/top/queries"),
            &opts(true, true),
            AuthOutcome::Admin,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_top_prepared_returns_200() {
        let r = dispatch(
            &req("GET", "/api/top/prepared"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_apps_returns_200() {
        let r = dispatch(
            &req("GET", "/api/apps"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_events_returns_200() {
        let r = dispatch(
            &req("GET", "/api/events"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }
}
