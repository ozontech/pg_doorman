use arc_swap::ArcSwap;
use log::{LevelFilter, Log, Metadata, Record};
use once_cell::sync::OnceCell;
use std::sync::Arc;

/// Global controller instance, set once during init.
static CONTROLLER: OnceCell<&'static LogLevelController> = OnceCell::new();

/// Runtime log level controller.
///
/// Wraps the real logger (tracing bridge or syslog) and adds per-module
/// filtering that can be changed at runtime via admin `SET log_level`.
///
/// Hot path cost at production level (INFO): zero — `log::max_level()`
/// atomic load short-circuits before our code runs. Per-module filtering
/// adds ~5ns per log call (one `ArcSwap::load` + prefix scan) only when
/// the global gate passes.
pub struct LogLevelController {
    inner: Box<dyn Log>,
    /// Per-module overrides sorted by prefix length (longest first).
    module_filters: ArcSwap<Vec<(String, LevelFilter)>>,
    /// Global base level (fallback when no module match).
    base_level: ArcSwap<LevelFilter>,
    /// Startup level for `SET log_level = 'default'`.
    startup_level: LevelFilter,
}

impl LogLevelController {
    pub fn new(inner: Box<dyn Log>, startup_level: LevelFilter) -> Self {
        Self {
            inner,
            module_filters: ArcSwap::from_pointee(Vec::new()),
            base_level: ArcSwap::from_pointee(startup_level),
            startup_level,
        }
    }

    /// Register as the global controller. Called once during init.
    pub fn register(self) {
        let controller: &'static LogLevelController = Box::leak(Box::new(self));
        let level = **controller.base_level.load();
        log::set_logger(controller).unwrap();
        log::set_max_level(level);
        CONTROLLER.set(controller).ok();
    }
}

impl Log for LogLevelController {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let base = **self.base_level.load();
        if metadata.level() <= base {
            return true;
        }
        // Level exceeds base — check per-module overrides
        let filters = self.module_filters.load();
        if filters.is_empty() {
            return false;
        }
        let target = metadata.target();
        for (prefix, level) in filters.iter() {
            if target.starts_with(prefix.as_str()) {
                return metadata.level() <= *level;
            }
        }
        false
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

// ---------------------------------------------------------------------------
// Public API for admin SET / SHOW
// ---------------------------------------------------------------------------

/// Apply a new log filter at runtime. Accepts RUST_LOG syntax:
/// - `"info"` — global level
/// - `"warn,pg_doorman::pool=debug"` — global warn + module override
/// - `"default"` — reset to startup level
pub fn set_log_level(filter_str: &str) -> Result<(), String> {
    let controller = CONTROLLER.get().ok_or("Logger not initialized")?;

    let filter_str = filter_str.trim().trim_matches('\'').trim_matches('"');

    if filter_str.eq_ignore_ascii_case("default") {
        controller
            .base_level
            .store(Arc::new(controller.startup_level));
        controller.module_filters.store(Arc::new(Vec::new()));
        log::set_max_level(controller.startup_level);
        return Ok(());
    }

    let (base, modules) = parse_filter(filter_str)?;

    // max_level must be the most permissive across base + all module overrides
    let max_level = modules
        .iter()
        .map(|(_, l)| *l)
        .chain(std::iter::once(base))
        .max()
        .unwrap_or(base);

    controller.base_level.store(Arc::new(base));
    controller.module_filters.store(Arc::new(modules));
    log::set_max_level(max_level);

    Ok(())
}

/// Return the current log filter as a human-readable string.
pub fn get_log_level() -> String {
    let Some(controller) = CONTROLLER.get() else {
        return "unknown".to_string();
    };

    let base = **controller.base_level.load();
    let filters = controller.module_filters.load();

    if filters.is_empty() {
        return level_to_str(base).to_string();
    }

    let mut parts = vec![level_to_str(base).to_string()];
    for (prefix, level) in filters.iter() {
        parts.push(format!("{}={}", prefix, level_to_str(*level)));
    }
    parts.join(",")
}

// ---------------------------------------------------------------------------
// Filter string parser (RUST_LOG subset)
// ---------------------------------------------------------------------------

fn parse_filter(s: &str) -> Result<(LevelFilter, Vec<(String, LevelFilter)>), String> {
    let mut base = None;
    let mut modules = Vec::new();

    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((module, level_str)) = part.split_once('=') {
            let module = module.trim();
            let level = parse_level(level_str.trim())?;
            if module.len() > 256 {
                return Err(format!("Module path too long: {}", &module[..64]));
            }
            modules.push((module.to_string(), level));
        } else {
            // Bare level — global base
            let level = parse_level(part)?;
            if base.is_some() {
                return Err("Multiple base levels specified".to_string());
            }
            base = Some(level);
        }
    }

    // Sort modules by prefix length descending (longest match first)
    modules.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    Ok((base.unwrap_or(LevelFilter::Info), modules))
}

fn parse_level(s: &str) -> Result<LevelFilter, String> {
    match s.to_ascii_lowercase().as_str() {
        "error" => Ok(LevelFilter::Error),
        "warn" | "warning" => Ok(LevelFilter::Warn),
        "info" => Ok(LevelFilter::Info),
        "debug" => Ok(LevelFilter::Debug),
        "trace" => Ok(LevelFilter::Trace),
        "off" => Ok(LevelFilter::Off),
        _ => Err(format!(
            "Invalid log level '{}'. Valid: error, warn, info, debug, trace, off",
            s
        )),
    }
}

fn level_to_str(level: LevelFilter) -> &'static str {
    match level {
        LevelFilter::Off => "off",
        LevelFilter::Error => "error",
        LevelFilter::Warn => "warn",
        LevelFilter::Info => "info",
        LevelFilter::Debug => "debug",
        LevelFilter::Trace => "trace",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_filter_global_only() {
        let (base, modules) = parse_filter("info").unwrap();
        assert_eq!(base, LevelFilter::Info);
        assert!(modules.is_empty());
    }

    #[test]
    fn test_parse_filter_global_with_modules() {
        let (base, modules) =
            parse_filter("warn,pg_doorman::pool=debug,pg_doorman::auth::scram=trace").unwrap();
        assert_eq!(base, LevelFilter::Warn);
        assert_eq!(modules.len(), 2);
        // Sorted by prefix length descending (longest first for match priority)
        assert_eq!(modules[0].0, "pg_doorman::auth::scram");
        assert_eq!(modules[0].1, LevelFilter::Trace);
        assert_eq!(modules[1].0, "pg_doorman::pool");
        assert_eq!(modules[1].1, LevelFilter::Debug);
    }

    #[test]
    fn test_parse_filter_modules_only() {
        let (base, modules) = parse_filter("pg_doorman::pool=debug").unwrap();
        assert_eq!(base, LevelFilter::Info); // default
        assert_eq!(modules.len(), 1);
    }

    #[test]
    fn test_parse_filter_invalid_level() {
        assert!(parse_filter("garbage").is_err());
    }

    #[test]
    fn test_parse_filter_multiple_base_levels() {
        assert!(parse_filter("info,debug").is_err());
    }

    #[test]
    fn test_parse_level_case_insensitive() {
        assert_eq!(parse_level("DEBUG").unwrap(), LevelFilter::Debug);
        assert_eq!(parse_level("Warning").unwrap(), LevelFilter::Warn);
    }

    #[test]
    fn test_level_to_str_roundtrip() {
        for level in [
            LevelFilter::Error,
            LevelFilter::Warn,
            LevelFilter::Info,
            LevelFilter::Debug,
            LevelFilter::Trace,
            LevelFilter::Off,
        ] {
            assert_eq!(parse_level(level_to_str(level)).unwrap(), level);
        }
    }

    #[test]
    fn test_get_log_level_format() {
        // Can't test with real CONTROLLER (OnceCell), but test the formatting logic
        let base = LevelFilter::Warn;
        let filters = vec![("pg_doorman::pool".to_string(), LevelFilter::Debug)];
        let mut parts = vec![level_to_str(base).to_string()];
        for (prefix, level) in &filters {
            parts.push(format!("{}={}", prefix, level_to_str(*level)));
        }
        assert_eq!(parts.join(","), "warn,pg_doorman::pool=debug");
    }
}
