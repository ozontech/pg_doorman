mod doorman_helper;
mod extended;
mod log_helper;
mod postgres_helper;
mod shell_helper;
mod world;

use cucumber::World;
use world::DoormanWorld;

/// Cleanup function to kill any remaining pg_doorman processes
/// This is called after tests complete (success or failure) to ensure
/// no zombie pg_doorman processes remain running
fn cleanup_pg_doorman_processes() {
    use std::process::Command;

    // Get the path to pg_doorman binary
    let doorman_binary = env!("CARGO_BIN_EXE_pg_doorman");

    // Find and kill any pg_doorman processes started by this test run
    // Use pkill with the full path to be more specific
    #[cfg(unix)]
    {
        // First try to kill by exact binary path
        let _ = Command::new("pkill")
            .arg("-9")
            .arg("-f")
            .arg(doorman_binary)
            .status();

        // Also kill any pg_doorman processes by name as fallback
        let _ = Command::new("pkill").arg("-9").arg("pg_doorman").status();
    }
}

/// Custom panic hook that ensures cleanup runs on panic
fn setup_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Run cleanup before default panic handling
        cleanup_pg_doorman_processes();
        // Call the default panic hook
        default_hook(panic_info);
    }));
}

fn main() {
    // Setup panic hook to ensure cleanup on panic
    setup_panic_hook();

    // Create tokio runtime manually so we can control cleanup
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Run tests with after hook for cleanup
    // Note: run_and_exit() will call std::process::exit() with appropriate exit code
    // (non-zero if any tests failed), so cleanup must happen in the after hook
    rt.block_on(async {
        DoormanWorld::cucumber()
            .after(|_feature, _rule, _scenario, _finished, world| {
                // This hook is called after EVERY scenario, regardless of success/failure
                // Cleanup pg_doorman process if it exists
                // NOTE: We only stop the specific process from this scenario, NOT all pg_doorman processes
                // because the next scenario's Background steps may have already started a new pg_doorman
                if let Some(w) = world {
                    if let Some(ref mut child) = w.doorman_process {
                        doorman_helper::stop_doorman(child);
                    }
                    w.doorman_process = None;
                }
                Box::pin(async {})
            })
            // run_and_exit() exits with non-zero code if any tests failed
            .run_and_exit("tests/bdd/features")
            .await;
    });

    // This code is unreachable because run_and_exit() calls std::process::exit()
    // but we keep it as a safety net in case the behavior changes
    cleanup_pg_doorman_processes();
}
