pub fn truncate_log(s: &str) -> String {
    let limit = 2000;
    if s.len() <= limit {
        s.to_string()
    } else {
        format!("... [truncated] ...\n{}", &s[s.len() - limit..])
    }
}
