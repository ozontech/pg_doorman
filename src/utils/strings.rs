//! Shared string-truncation helpers. Three call sites — Patroni REST
//! error rendering, query previews for admin/UI, and per-eviction log
//! lines — used to maintain their own inline copies; this module is the
//! one place where the byte/char limits live.

/// Maximum number of characters (not bytes — query text may contain
/// multi-byte UTF-8) kept when a query is rendered into a log line.
/// Long queries are truncated with an ellipsis so a runaway statement
/// can't blow up a log shipper.
pub const LOG_QUERY_MAX_CHARS: usize = 80;

/// Maximum characters preserved in API/admin previews of a query
/// (`/api/top/queries`, `/api/interner`, `SHOW QUERIES`). Wider than
/// the log line because the consumer is interactive UI / `psql` output
/// that can wrap rather than a single-line shipper.
pub const PREVIEW_QUERY_MAX_CHARS: usize = 120;

/// Truncate `s` to at most `max_bytes`, walking back to the nearest
/// UTF-8 char boundary so the slice is always valid UTF-8. Zero-copy.
/// Use this when the limit is a wire-protocol or log-shipper byte cap
/// (Patroni REST error bodies, syslog line size) and you don't need
/// an ellipsis.
pub fn truncate_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Take the first `max_chars` characters of `s`. Allocates a fresh
/// `String`. No ellipsis, no normalisation. Building block for the
/// query-rendering helpers below.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// Render a query string into a compact form safe for a single log
/// line: CR/LF collapsed to spaces and trimmed to `LOG_QUERY_MAX_CHARS`
/// characters with a trailing "..." when truncated. Always allocates;
/// the `log` crate's macros already short-circuit argument evaluation
/// below the active level, so a bare `trace!(...)` call is enough —
/// explicit `log_enabled!` guards are only useful on hot paths where
/// avoiding the allocation matters.
pub fn truncate_query_for_log(query: &str) -> String {
    let cleaned = query.replace(['\n', '\r'], " ");
    if cleaned.chars().count() <= LOG_QUERY_MAX_CHARS {
        return cleaned;
    }
    let mut out = truncate_chars(&cleaned, LOG_QUERY_MAX_CHARS);
    out.push_str("...");
    out
}

/// First `PREVIEW_QUERY_MAX_CHARS` characters of `query`, verbatim. No
/// ellipsis, no newline collapse — preview surfaces (admin SHOW,
/// `/api/top/queries`, `/api/interner`) are expected to render the
/// text as-is so operators can read the original statement.
pub fn preview_query(query: &str) -> String {
    truncate_chars(query, PREVIEW_QUERY_MAX_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_bytes_passthrough_when_within_limit() {
        assert_eq!(truncate_bytes("hello", 10), "hello");
    }

    #[test]
    fn truncate_bytes_at_exact_limit() {
        assert_eq!(truncate_bytes("hello", 5), "hello");
    }

    #[test]
    fn truncate_bytes_walks_back_to_char_boundary() {
        // Cyrillic 'я' is 2 bytes (0xD1 0x8F). Limit 3 sits mid-char of
        // the second letter; truncate_bytes must back off to 2.
        let s = "яя";
        assert_eq!(s.len(), 4);
        assert_eq!(truncate_bytes(s, 3), "я");
    }

    #[test]
    fn truncate_bytes_zero_limit() {
        assert_eq!(truncate_bytes("abc", 0), "");
    }

    #[test]
    fn truncate_chars_caps_at_limit() {
        let q: String = "a".repeat(10);
        assert_eq!(truncate_chars(&q, 5), "aaaaa");
    }

    #[test]
    fn truncate_chars_passthrough_when_within_limit() {
        assert_eq!(truncate_chars("abc", 10), "abc");
    }

    #[test]
    fn truncate_chars_counts_multi_byte_as_one() {
        // Five 'я' chars = 10 bytes; truncate_chars(_, 5) keeps all five.
        let q = "я".repeat(5);
        let out = truncate_chars(&q, 5);
        assert_eq!(out.chars().count(), 5);
        assert_eq!(out, q);
    }

    #[test]
    fn truncate_query_for_log_keeps_short_query_intact() {
        assert_eq!(truncate_query_for_log("select 1"), "select 1");
    }

    #[test]
    fn truncate_query_for_log_collapses_newlines_to_spaces() {
        assert_eq!(
            truncate_query_for_log("select\n1\rfrom\r\nt"),
            "select 1 from  t"
        );
    }

    #[test]
    fn truncate_query_for_log_no_ellipsis_at_exact_limit() {
        let q: String = "a".repeat(LOG_QUERY_MAX_CHARS);
        let out = truncate_query_for_log(&q);
        assert_eq!(out.chars().count(), LOG_QUERY_MAX_CHARS);
        assert!(!out.ends_with("..."));
    }

    #[test]
    fn truncate_query_for_log_appends_ellipsis_past_limit() {
        let q: String = "a".repeat(LOG_QUERY_MAX_CHARS + 5);
        let out = truncate_query_for_log(&q);
        assert!(out.ends_with("..."));
        assert_eq!(out.chars().count(), LOG_QUERY_MAX_CHARS + 3);
    }

    #[test]
    fn truncate_query_for_log_empty_input() {
        assert_eq!(truncate_query_for_log(""), "");
    }

    #[test]
    fn truncate_query_for_log_truncates_by_chars_not_bytes() {
        // Multi-byte chars: Cyrillic 'а' is 2 bytes. LOG_QUERY_MAX_CHARS
        // characters of 'а' is 2 * LOG_QUERY_MAX_CHARS bytes; truncation
        // must count chars, not bytes — otherwise a UTF-8 boundary slice
        // panics or produces invalid UTF-8.
        let q: String = "а".repeat(LOG_QUERY_MAX_CHARS + 10);
        let out = truncate_query_for_log(&q);
        assert!(out.ends_with("..."));
        assert_eq!(out.chars().count(), LOG_QUERY_MAX_CHARS + 3);
    }

    #[test]
    fn preview_query_keeps_short_input_untouched() {
        assert_eq!(preview_query("SELECT 1\nFROM t"), "SELECT 1\nFROM t");
    }

    #[test]
    fn preview_query_caps_at_preview_max_no_ellipsis() {
        let q: String = "a".repeat(PREVIEW_QUERY_MAX_CHARS + 50);
        let out = preview_query(&q);
        assert_eq!(out.chars().count(), PREVIEW_QUERY_MAX_CHARS);
        assert!(!out.ends_with("..."));
    }

    #[test]
    fn preview_query_handles_multi_byte_chars() {
        let q: String = "ы".repeat(PREVIEW_QUERY_MAX_CHARS + 5);
        let out = preview_query(&q);
        assert_eq!(out.chars().count(), PREVIEW_QUERY_MAX_CHARS);
    }
}
