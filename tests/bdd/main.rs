mod doorman_helper;
mod extended;
mod pg_connection;
mod postgres_helper;
mod shell_helper;
mod world;

use cucumber::World;
use world::DoormanWorld;

fn main() {
    // Initialize tracing subscriber for debug logging when DEBUG env var is set
    if std::env::var("DEBUG").is_ok() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_target(true)
            .with_thread_ids(true)
            .with_line_number(true)
            .init();
    }

    // Create tokio runtime manually so we can control cleanup
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Run tests with after hook for cleanup
    rt.block_on(async {
        // Parse CLI options and add todo-skip filter
        use cucumber::gherkin::tagexpr::TagOperation;
        let mut cli = cucumber::cli::Opts::<
            cucumber::parser::basic::Cli,
            cucumber::runner::basic::Cli,
            cucumber::writer::basic::Cli,
            cucumber::cli::Empty,
        >::parsed();

        // Create "not @todo-skip" filter
        let not_todo_skip = TagOperation::Not(Box::new(TagOperation::Tag("todo-skip".to_string())));

        // Combine with existing tags filter if present
        cli.tags_filter = match cli.tags_filter.take() {
            Some(existing) => Some(TagOperation::And(
                Box::new(existing),
                Box::new(not_todo_skip),
            )),
            None => Some(not_todo_skip),
        };

        let writer = DoormanWorld::cucumber()
            .max_concurrent_scenarios(5)
            .with_cli(cli)
            .before(|_feature, _rule, scenario, _world| {
                Box::pin(async move {
                    // Spawn a timeout task for this scenario
                    let scenario_name = scenario.name.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                        eprintln!(
                            "⚠️  Scenario '{}' exceeded 300 second timeout",
                            scenario_name
                        );
                        std::process::exit(124); // Timeout exit code
                    });
                })
            })
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
