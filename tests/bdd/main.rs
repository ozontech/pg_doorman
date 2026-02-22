mod doorman_helper;
mod extended;
mod fuzz_client;
mod fuzz_helper;
mod generate_helper;
mod odyssey_helper;
mod pg_connection;
mod pgbench_helper;
mod pgbouncer_helper;
mod pool_bench_helper;
mod postgres_helper;
mod service_helper;
mod shell_helper;
mod utils;
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
    // Use 4 worker threads explicitly for consistent benchmark results
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();

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
            .max_concurrent_scenarios(1)
            .fail_fast()
            .with_cli(cli)
            .before(|feature, _rule, scenario, world| {
                Box::pin(async move {
                    // Skip timeout for @bench scenarios - they run long benchmarks
                    let is_bench = feature.tags.iter().any(|t| t == "bench")
                        || scenario.tags.iter().any(|t| t == "bench");
                    if is_bench {
                        eprintln!(
                            "ℹ️  Scenario '{}' is a benchmark, timeout disabled",
                            scenario.name
                        );
                        world.is_bench = true;
                        return;
                    }

                    // Spawn a periodic warning task for slow scenarios
                    // Prints every 60 seconds while test is running, aborted when scenario finishes
                    let scenario_name = scenario.name.clone();
                    let feature_name = feature.name.clone();
                    let slow_warning_task = tokio::spawn(async move {
                        let mut elapsed_minutes = 0u64;
                        loop {
                            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                            elapsed_minutes += 1;
                            eprintln!(
                                "\n🐢🐢🐢 SLOW TEST 🐢🐢🐢\n\
                                 >>> Test '{scenario_name}' (feature '{feature_name}') is RUNNING for {elapsed_minutes} minute(s) <<<\n\
                                 🐢🐢🐢 SLOW TEST 🐢🐢🐢\n"
                            );
                        }
                    });
                    world.slow_warning_abort = Some(slow_warning_task.abort_handle());
                })
            })
            .after(|_feature, _rule, _scenario, _finished, world| {
                // This hook is called after EVERY scenario, regardless of success/failure
                // Cleanup pg_doorman process if it exists
                // NOTE: We only stop the specific process from this scenario, NOT all pg_doorman processes
                // because the next scenario's Background steps may have already started a new pg_doorman
                if let Some(w) = world {
                    // Cancel the slow warning task since scenario has finished
                    if let Some(abort_handle) = w.slow_warning_abort.take() {
                        abort_handle.abort();
                    }
                    if let Some(ref mut child) = w.doorman_process {
                        doorman_helper::stop_doorman(child);
                    }
                    w.doorman_process = None;

                    // Stop daemon process if running (for daemon mode tests)
                    // Read PID from file to handle binary-upgrade where PID changes
                    if let Some(ref pid_path) = w.doorman_daemon_pid_file {
                        if let Ok(pid_content) = std::fs::read_to_string(pid_path) {
                            if let Ok(pid) = pid_content.trim().parse::<u32>() {
                                doorman_helper::stop_doorman_daemon(pid);
                            }
                        }
                    }
                    w.doorman_daemon_pid_file = None;

                    if let Some(ref mut child) = w.pgbouncer_process {
                        pgbouncer_helper::stop_pgbouncer(child);
                    }
                    w.pgbouncer_process = None;

                    if let Some(ref mut child) = w.odyssey_process {
                        odyssey_helper::stop_odyssey(child);
                    }
                    w.odyssey_process = None;
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
