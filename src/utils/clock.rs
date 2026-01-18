use once_cell::sync::Lazy;
use quanta::Clock;

/// Global high-performance clock for hot-path timing.
/// Uses TSC (Time Stamp Counter) on x86/x86_64 for minimal overhead.
pub static CLOCK: Lazy<Clock> = Lazy::new(Clock::new);

/// Get current instant (precise, ~10ns overhead).
/// Use for critical timing where precision matters.
#[inline]
pub fn now() -> quanta::Instant {
    CLOCK.now()
}

/// Get recent instant (cached, ~1-2ns overhead).
/// Use for statistics and metrics where speed > precision.
#[inline]
pub fn recent() -> quanta::Instant {
    CLOCK.recent()
}
