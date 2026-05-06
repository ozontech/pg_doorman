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

use log::{error, info};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{TcpSocket, TcpStream};

use crate::web::auth::{classify, AuthOutcome};
use crate::web::metrics::write_metrics_response;

/// Runtime state needed by the mux on every request.
#[derive(Clone)]
pub struct WebServerOptions {
    /// `true` when `[web].ui = true` AND admin_password is non-default.
    /// When `false`, the listener serves only `/metrics`; everything else → 404.
    pub ui_active: bool,
    /// `[web].ui_anonymous` (gates public `/api/*` and SPA paths when ui_active).
    pub ui_anonymous: bool,
    pub admin_username: String,
    pub admin_password: String,
}

/// Admin-only path prefixes (require `Admin` auth regardless of `ui_anonymous`).
/// Spec section 6.1.
const ADMIN_ONLY_PREFIXES: &[&str] = &["/api/logs", "/api/prepared/text/", "/api/interner/top"];

/// Spawns the HTTP listener for the given address.
pub async fn start_web_server(host: &str, opts: WebServerOptions) {
    info!("starting web listener on {host}");
    let addr: SocketAddr = match host.parse() {
        Ok(addr) => addr,
        Err(e) => panic!("Failed to parse socket address '{host}': {e}"),
    };

    let listen_socket = if addr.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }
    .unwrap_or_else(|e| panic!("Failed to create socket: {e}"));

    listen_socket
        .set_reuseaddr(true)
        .unwrap_or_else(|e| panic!("Failed to set SO_REUSEADDR: {e}"));
    listen_socket
        .set_reuseport(true)
        .unwrap_or_else(|e| panic!("Failed to set SO_REUSEPORT: {e}"));
    listen_socket
        .bind(addr)
        .unwrap_or_else(|e| panic!("Failed to bind to address {addr}: {e}"));

    let listener = listen_socket
        .listen(1024)
        .unwrap_or_else(|e| panic!("Failed to listen on {addr}: {e}"));
    info!("web listener bound on {addr}");

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let opts = opts.clone();
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

async fn handle_connection(stream: TcpStream, opts: WebServerOptions) {
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut writer = BufWriter::new(write_half);

    let mut buf = [0u8; 4096];
    let n = match reader.read(&mut buf).await {
        Ok(0) => return,
        Ok(n) => n,
        Err(e) => {
            error!("Failed to read HTTP request: {e}");
            return;
        }
    };

    let raw = match std::str::from_utf8(&buf[..n]) {
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

    // /metrics is always served, regardless of ui_active or auth.
    if parsed.method == "GET" && parsed.path == "/metrics" {
        let accepts_gzip = parsed.accepts_gzip;
        write_metrics_response(&mut writer, accepts_gzip).await;
        return;
    }

    let auth = classify(
        parsed.authorization,
        &opts.admin_username,
        &opts.admin_password,
    );

    let response = dispatch(&parsed, &opts, auth);
    let _ = response.write(&mut writer).await;
}

#[derive(Debug)]
struct ParsedRequest<'a> {
    method: &'a str,
    path: &'a str,
    authorization: Option<&'a str>,
    accepts_gzip: bool,
}

impl<'a> ParsedRequest<'a> {
    fn parse(raw: &'a str) -> Option<Self> {
        let mut lines = raw.split("\r\n");
        let request_line = lines.next()?;
        let mut parts = request_line.splitn(3, ' ');
        let method = parts.next()?;
        let path = parts.next()?;
        let _http_version = parts.next()?;

        let mut authorization = None;
        let mut accepts_gzip = false;
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
            }
        }
        Some(ParsedRequest {
            method,
            path,
            authorization,
            accepts_gzip,
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

    pub(crate) fn unauthorized() -> Self {
        let mut r = Response::status(401, "Unauthorized");
        r.extra_headers.push((
            "WWW-Authenticate",
            "Basic realm=\"pg_doorman admin\"".into(),
        ));
        r
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

fn route_api(req: &ParsedRequest<'_>) -> Response {
    use crate::web::routes;
    use crate::web::routes::query::parse_query;

    let (path, query_str) = match req.path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (req.path, ""),
    };
    let query = parse_query(query_str);

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
        _ => Response::json(
            501,
            "Not Implemented",
            r#"{"error":"not_implemented","message":"endpoint will be wired in a later phase"}"#,
        ),
    }
}

fn dispatch(req: &ParsedRequest<'_>, opts: &WebServerOptions, auth: AuthOutcome) -> Response {
    if req.method != "GET" && req.method != "HEAD" {
        return Response::status(405, "Method Not Allowed");
    }

    if !opts.ui_active {
        // /metrics already handled before dispatch().
        return Response::status(404, "Not Found");
    }

    let admin_only = req.path.starts_with("/api/") && is_admin_only(req.path);

    let needs_admin = admin_only || (!opts.ui_anonymous);
    if needs_admin && auth != AuthOutcome::Admin {
        return Response::unauthorized();
    }

    if req.path.starts_with("/api/") {
        return route_api(req);
    }

    if req.path == "/" || req.path.starts_with("/assets/") {
        // SPA bundle is wired in phase 7.
        return Response::status(404, "Not Found");
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
        }
    }

    #[test]
    fn parse_minimal_get() {
        let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert_eq!(p.method, "GET");
        assert_eq!(p.path, "/api/foo");
        assert_eq!(p.authorization, None);
        assert_eq!(p.accepts_gzip, false);
    }

    #[test]
    fn parse_with_authorization_header() {
        let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\nAuthorization: Basic abc\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert_eq!(p.authorization, Some("Basic abc"));
        assert_eq!(p.accepts_gzip, false);
    }

    #[test]
    fn parse_with_lowercase_authorization() {
        let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\nauthorization: Basic abc\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert_eq!(p.authorization, Some("Basic abc"));
        assert_eq!(p.accepts_gzip, false);
    }

    #[test]
    fn parse_rejects_malformed_request_line() {
        assert!(ParsedRequest::parse("garbage").is_none());
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

    #[test]
    fn dispatch_admin_path_with_admin_auth_returns_501() {
        let r = dispatch(
            &req("GET", "/api/logs"),
            &opts(true, true),
            AuthOutcome::Admin,
        );
        assert_eq!(r.status, 501);
    }

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
    fn dispatch_404_for_root_in_phase_2() {
        let r = dispatch(&req("GET", "/"), &opts(true, true), AuthOutcome::Admin);
        assert_eq!(r.status, 404);
    }

    #[test]
    fn dispatch_404_for_assets_in_phase_2() {
        let r = dispatch(
            &req("GET", "/assets/main.js"),
            &opts(true, true),
            AuthOutcome::Admin,
        );
        assert_eq!(r.status, 404);
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
}
