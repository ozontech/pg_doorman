//! SPA bundle embedded into the binary.
//!
//! `frontend/dist/` is checked into the repo (Web UI design decision #22) and
//! pulled in here at compile time via `include_dir!`. The lookup helper
//! returns the file contents and a Content-Type for the requested URL path,
//! falling back to `index.html` for any URL that does not match a real asset
//! — the SPA owns its own routing, so a deep-link like `/pools` should
//! resolve to the SPA shell, not 404.
//!
//! The empty-bundle case (someone built the workspace without ever running
//! `npm run build` in `frontend/`) is handled gracefully: callers receive
//! `None` and the mux returns 404, mirroring the pre-embedding behaviour.

use include_dir::{include_dir, Dir};

static SPA: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/frontend/dist");

/// Asset payload returned to the mux: bytes, mime type, and whether the asset
/// is content-hashed (immutable forever) so the mux can pick the right
/// Cache-Control header. `path` is the bundle-relative file name; the gzip
/// cache uses it as a stable key so the per-asset compressed body is
/// computed once and reused across requests.
pub(crate) struct Asset {
    pub path: &'static str,
    pub bytes: &'static [u8],
    pub mime: &'static str,
    pub immutable: bool,
}

/// Looks up the request path inside the embedded bundle.
///
/// Returns `None` when the bundle is empty (no `index.html`); this lets the
/// caller emit a 404 instead of an empty success.
pub(crate) fn lookup(path: &str) -> Option<Asset> {
    if !has_index() {
        return None;
    }

    let stripped = path.trim_start_matches('/');
    let target = if let Some(file) = SPA.get_file(stripped) {
        file
    } else {
        // SPA fallback: any URL that does not match a real asset returns the
        // shell so client-side routing can take over. Real 404s are reserved
        // for `/api/*` and `/metrics`.
        SPA.get_file("index.html")?
    };

    // `target.path()` returns `&'a Path` with `'a` tied to the embedded
    // `Dir<'static>`, so `to_str()` likewise yields `&'static str`.
    // Spelling the lifetimes out explicitly lets us hand the path to
    // the gzip cache as a `&'static str` key without `unsafe`.
    let target_path: &'static std::path::Path = target.path();
    let path_str: &'static str = target_path.to_str().unwrap_or("");
    let mime = mime_for(path_str);
    let immutable = path_str.starts_with("assets/");
    Some(Asset {
        path: path_str,
        bytes: target.contents(),
        mime,
        immutable,
    })
}

fn has_index() -> bool {
    SPA.get_file("index.html").is_some()
}

fn mime_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" | "map" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_for_known_extensions() {
        assert_eq!(mime_for("a.html"), "text/html; charset=utf-8");
        assert_eq!(mime_for("a.js"), "application/javascript; charset=utf-8");
        assert_eq!(mime_for("a.css"), "text/css; charset=utf-8");
        assert_eq!(mime_for("a.svg"), "image/svg+xml");
        assert_eq!(mime_for("a.woff2"), "font/woff2");
    }

    #[test]
    fn mime_for_unknown_falls_back_to_octet_stream() {
        assert_eq!(mime_for("a.xyz"), "application/octet-stream");
        assert_eq!(mime_for("noext"), "application/octet-stream");
    }

    #[test]
    fn lookup_returns_index_for_root() {
        // The bundle is committed in this repo, so this should resolve.
        let asset = lookup("/");
        assert!(asset.is_some(), "lookup('/') should hit the SPA shell");
        let a = asset.unwrap();
        assert!(!a.immutable);
        assert_eq!(a.mime, "text/html; charset=utf-8");
    }

    #[test]
    fn lookup_falls_back_to_index_for_unknown_path() {
        let asset = lookup("/pools/foo/bar").expect("fallback to index");
        assert_eq!(asset.mime, "text/html; charset=utf-8");
    }

    #[test]
    fn lookup_returns_assets_with_immutable_marker() {
        let asset_dir = SPA.get_dir("assets").expect("dist must contain assets/");
        let any = asset_dir
            .files()
            .next()
            .expect("dist/assets/ must contain at least one file");
        let req = format!("/{}", any.path().to_string_lossy());
        let a = lookup(&req).expect("lookup hashed asset");
        assert!(a.immutable, "asset under assets/ should be immutable");
    }
}
