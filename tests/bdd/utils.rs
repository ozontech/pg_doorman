use std::io::Write;
use std::process::Stdio;
use tempfile::NamedTempFile;

/// Check if the current process is running as root (effective UID)
pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Get stdio configuration based on DEBUG environment variable
pub fn get_stdio_config() -> (Stdio, Stdio) {
    if std::env::var("DEBUG").is_ok() {
        (Stdio::inherit(), Stdio::inherit())
    } else {
        (Stdio::null(), Stdio::null())
    }
}

/// Check if DEBUG mode is enabled
pub fn is_debug_mode() -> bool {
    std::env::var("DEBUG").is_ok()
}

/// Create a temporary file with the given content
pub fn create_temp_file(content: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    file.write_all(content.as_bytes())
        .expect("Failed to write content to temp file");
    file.flush().expect("Failed to flush temp file");
    file
}

/// Set file permissions (unix only)
#[cfg(unix)]
pub fn set_file_permissions(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .expect("Failed to set file permissions");
}
