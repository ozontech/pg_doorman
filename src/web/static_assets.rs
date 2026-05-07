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

/// Asset payload returned to the mux. `bytes` is whatever the bundle holds
/// for the matched URL — gzipped when `pre_gzipped == true` (the build step
/// pre-compresses every text-like file), raw otherwise. `mime` and
/// `immutable` describe the *decompressed* identity, so a `.js.gz` blob
/// still reports `application/javascript` and lives under `assets/`.
pub(crate) struct Asset {
    pub bytes: &'static [u8],
    pub mime: &'static str,
    pub immutable: bool,
    /// `true` when `bytes` is the gzip-compressed form of the asset. The
    /// caller either serves it directly with `Content-Encoding: gzip` or
    /// decompresses on the fly for clients that do not advertise gzip.
    pub pre_gzipped: bool,
}

/// Looks up the request path inside the embedded bundle.
///
/// Tries the exact path first, then `{path}.gz` for the pre-compressed form
/// the build pipeline writes. Falls back to `index.html(.gz)` so the SPA
/// owns deep-link routing — only `/api/*` and `/metrics` ever return a real
/// 404. Returns `None` when the bundle is empty (no `index.html` at all).
pub(crate) fn lookup(path: &str) -> Option<Asset> {
    if !has_index() {
        return None;
    }

    let stripped = path.trim_start_matches('/');
    if let Some(asset) = lookup_exact(stripped) {
        return Some(asset);
    }
    // SPA fallback for client-side routes (`/pools`, `/clients/...`).
    lookup_exact("index.html")
}

/// Try the literal path first, then the `.gz` neighbour. Helpers below
/// keep the static lifetimes explicit so a refactor cannot accidentally
/// borrow from the request String.
fn lookup_exact(stripped: &str) -> Option<Asset> {
    if let Some(file) = SPA.get_file(stripped) {
        return Some(asset_for(file, false));
    }
    let gz_path = format!("{stripped}.gz");
    SPA.get_file(&gz_path).map(|f| asset_for(f, true))
}

fn asset_for(file: &'static include_dir::File<'static>, pre_gzipped: bool) -> Asset {
    let target_path: &'static std::path::Path = file.path();
    let raw_path: &'static str = target_path.to_str().unwrap_or("");
    // For the gzipped variant the SPA path ends in ".gz"; the wire-level
    // identity (mime, immutability marker) reflects the original extension,
    // so strip the suffix when classifying.
    let identity_path: &'static str = if pre_gzipped {
        raw_path.strip_suffix(".gz").unwrap_or(raw_path)
    } else {
        raw_path
    };
    Asset {
        bytes: file.contents(),
        mime: mime_for(identity_path),
        immutable: identity_path.starts_with("assets/"),
        pre_gzipped,
    }
}

fn has_index() -> bool {
    SPA.get_file("index.html").is_some() || SPA.get_file("index.html.gz").is_some()
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
