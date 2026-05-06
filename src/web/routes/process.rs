//! `GET /api/process` — process resource snapshot. Linux reads
//! `/proc/self/*`. macOS / others fill what they can from the existing
//! `get_process_memory_usage()` and otherwise zero out fields the operator
//! tile must still draw a card for. The route is `pub(crate)` and dispatched
//! from `web::server::route_api`.

use crate::web::server::Response;

use super::collect::collect_process;

pub(crate) fn handle_process() -> Response {
    Response::ok_json(&collect_process())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_response_envelope_shape() {
        let r = handle_process();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        for field in [
            "\"ts\"",
            "\"pid\"",
            "\"hostname\"",
            "\"uptime_seconds\"",
            "\"rss_bytes\"",
            "\"cpu_user_us\"",
            "\"cpu_system_us\"",
            "\"threads_breakdown\"",
        ] {
            assert!(body.contains(field), "missing {field} in {body}");
        }
    }

}
