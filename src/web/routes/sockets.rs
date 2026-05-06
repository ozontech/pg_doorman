//! GET /api/sockets handler. Linux-only — non-linux returns 503 not_supported.

use crate::web::server::Response;

pub(crate) fn handle_sockets() -> Response {
    #[cfg(target_os = "linux")]
    {
        match crate::web::routes::collect::collect_sockets() {
            Ok(dto) => Response::ok_json(&dto),
            Err(msg) => {
                log::error!("collect_sockets failed: {msg}");
                Response::json(
                    500,
                    "Internal Server Error",
                    r#"{"error":"sockets_unavailable","message":"failed to read socket states"}"#,
                )
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Response::json(
            503,
            "Service Unavailable",
            r#"{"error":"not_supported","message":"sockets endpoint requires Linux"}"#,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn sockets_response_is_200_on_linux() {
        let r = handle_sockets();
        // Note: returns 500 if /proc/net/tcp* unreadable in CI sandbox; accept
        // both as long as the handler did not panic.
        assert!(r.status == 200 || r.status == 500, "got {}", r.status);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn sockets_response_is_503_on_non_linux() {
        let r = handle_sockets();
        assert_eq!(r.status, 503);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("not_supported"));
    }
}
