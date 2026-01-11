use std::io::Write;
use std::process::Stdio;
use tempfile::NamedTempFile;

/// Create a temporary file with the given content
pub fn create_temp_file(content: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    file.write_all(content.as_bytes())
        .expect("Failed to write to temp file");
    file.flush().expect("Failed to flush temp file");
    file
}

/// Check if DEBUG environment variable is set
pub fn is_debug_mode() -> bool {
    std::env::var("DEBUG").is_ok()
}

/// Get stdio configuration based on debug mode
pub fn get_stdio_config() -> (Stdio, Stdio) {
    if is_debug_mode() {
        (Stdio::inherit(), Stdio::inherit())
    } else {
        (Stdio::piped(), Stdio::piped())
    }
}
