//! Hand-rolled query string parser.
//!
//! Returns `BTreeMap<key, Vec<value>>` so multi-value keys (e.g.
//! `?application_name=a&application_name=b`) are preserved in order.
//! Keeps the dependency surface small (no `serde_urlencoded`).

use std::collections::BTreeMap;

pub fn parse_query(q: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if q.is_empty() {
        return out;
    }
    for part in q.split('&') {
        if part.is_empty() {
            continue;
        }
        let (k, v) = match part.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(part), String::new()),
        };
        out.entry(k).or_default().push(v);
    }
    out
}

fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '+' => out.push(' '),
            '%' => {
                let hi = chars.next();
                let lo = chars.next();
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    if let (Some(hi), Some(lo)) = (hi.to_digit(16), lo.to_digit(16)) {
                        out.push(((hi << 4 | lo) as u8) as char);
                        continue;
                    }
                }
            }
            other => out.push(other),
        }
    }
    out
}

pub fn first(map: &BTreeMap<String, Vec<String>>, key: &str) -> Option<String> {
    map.get(key).and_then(|v| v.first()).cloned()
}

pub fn parse_u64(map: &BTreeMap<String, Vec<String>>, key: &str, default: u64) -> u64 {
    first(map, key)
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_empty_map() {
        assert!(parse_query("").is_empty());
    }

    #[test]
    fn single_value() {
        let m = parse_query("limit=50");
        assert_eq!(m.get("limit"), Some(&vec!["50".to_string()]));
    }

    #[test]
    fn multiple_values_for_same_key() {
        let m = parse_query("application_name=a&application_name=b");
        assert_eq!(
            m.get("application_name"),
            Some(&vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn percent_decoding() {
        let m = parse_query("user=alice%40example");
        assert_eq!(m.get("user"), Some(&vec!["alice@example".to_string()]));
    }

    #[test]
    fn plus_to_space() {
        let m = parse_query("application_name=my+app");
        assert_eq!(m.get("application_name"), Some(&vec!["my app".to_string()]));
    }

    #[test]
    fn parse_u64_with_default() {
        let m = parse_query("limit=42");
        assert_eq!(parse_u64(&m, "limit", 100), 42);
        assert_eq!(parse_u64(&m, "missing", 100), 100);
    }
}
