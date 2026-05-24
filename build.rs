// Tell cargo to invalidate the build whenever the embedded SPA bundle
// changes. Without this hint, cargo only watches the regular source
// tree; a frontend rebuild that produces a new `frontend/dist/...js`
// would be silently ignored and the resulting binary would still embed
// the previous bundle. Docker builds share `target/`, so the COPY layer
// alone is too late to invalidate cargo.

use std::fs;
use std::path::Path;

fn main() {
    let dist = Path::new("frontend/dist");
    println!("cargo:rerun-if-changed=frontend/dist");
    println!("cargo:rerun-if-changed=frontend/dist/index.html");
    walk(dist);
}

fn walk(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Print every file so cargo's content-hash sees changes inside
        // hashed asset names too (a Vite rebuild renames the JS file
        // every time the source hash changes).
        let path_str = path.display().to_string();
        println!("cargo:rerun-if-changed={path_str}");
        if path.is_dir() {
            walk(&path);
        }
    }
}
