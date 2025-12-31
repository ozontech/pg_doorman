mod doorman_helper;
mod extended;
mod pg_connection;
mod postgres_helper;
mod shell_helper;
mod world;

use cucumber::World;
use world::DoormanWorld;

fn main() {

    // Create tokio runtime manually so we can control cleanup
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Run tests with after hook for cleanup
    rt.block_on(async {
        let writer = DoormanWorld::cucumber()
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
            .run("tests/bdd/features")
            .await;

        // Check if execution failed or if there are skipped tests
        use cucumber::writer::Stats;
        let has_failures = writer.execution_has_failed();
        let skipped = writer.skipped_steps();
        
        if has_failures || skipped > 0 {
            let mut msg = Vec::new();
            
            let failed_steps = writer.failed_steps();
            if failed_steps > 0 {
                msg.push(format!(
                    "{failed_steps} step{} failed",
                    if failed_steps > 1 { "s" } else { "" }
                ));
            }
            
            if skipped > 0 {
                msg.push(format!(
                    "{skipped} step{} skipped",
                    if skipped > 1 { "s" } else { "" }
                ));
            }
            
            let parsing_errors = writer.parsing_errors();
            if parsing_errors > 0 {
                msg.push(format!(
                    "{parsing_errors} parsing error{}",
                    if parsing_errors > 1 { "s" } else { "" }
                ));
            }
            
            let hook_errors = writer.hook_errors();
            if hook_errors > 0 {
                msg.push(format!(
                    "{hook_errors} hook error{}",
                    if hook_errors > 1 { "s" } else { "" }
                ));
            }
            
            eprintln!("Tests failed: {}", msg.join(", "));
            std::process::exit(1);
        }
    });
}
