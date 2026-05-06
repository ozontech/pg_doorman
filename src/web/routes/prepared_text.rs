//! GET /api/prepared/text/{hash} handler. Admin-only (mux gates the prefix).

use crate::web::routes::collect::collect_prepared_text;
use crate::web::server::Response;

pub(crate) fn handle_prepared_text(hash_str: &str) -> Response {
    let Some(hash) = parse_hash(hash_str) else {
        return Response::json(
            400,
            "Bad Request",
            r#"{"error":"bad_hash","message":"hash must be decimal or 0x-prefixed hex u64"}"#,
        );
    };
    match collect_prepared_text(hash) {
        Some(dto) => Response::ok_json(&dto),
        None => Response::json(
            404,
            "Not Found",
            r#"{"error":"not_found","message":"prepared statement not found for hash"}"#,
        ),
    }
}

fn parse_hash(s: &str) -> Option<u64> {
    if let Some(stripped) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u64::from_str_radix(stripped, 16).ok();
    }
    s.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_text_returns_404_on_unknown_hash() {
        let r = handle_prepared_text("0xdeadbeef");
        assert_eq!(r.status, 404);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("not_found"));
    }

    #[test]
    fn prepared_text_returns_400_on_malformed_hash() {
        let r = handle_prepared_text("not-a-hash");
        assert_eq!(r.status, 400);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("bad_hash"));
    }

    #[test]
    fn parse_hash_decimal() {
        assert_eq!(parse_hash("12345"), Some(12345));
    }

    #[test]
    fn parse_hash_hex_prefix() {
        assert_eq!(parse_hash("0xff"), Some(255));
        assert_eq!(parse_hash("0XFF"), Some(255));
    }

    #[test]
    fn parse_hash_invalid() {
        assert_eq!(parse_hash(""), None);
        assert_eq!(parse_hash("xyz"), None);
        assert_eq!(parse_hash("0xZZ"), None);
    }
}
