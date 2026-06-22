use crate::pg_connection::PgConnection;
use crate::world::DoormanWorld;
use cucumber::{given, then, when};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Owns the acceptor tasks started by `start_blackhole_listener` and
/// aborts them when `DoormanWorld` drops at the end of a scenario, so
/// scenarios cannot leak background tasks and file descriptors across
/// the suite.
#[derive(Default)]
pub struct BlackholeAbortHandles(pub Vec<JoinHandle<()>>);

impl Drop for BlackholeAbortHandles {
    fn drop(&mut self) {
        for handle in self.0.drain(..) {
            handle.abort();
        }
    }
}

/// Starts a TCP listener that accepts connections and never sends a
/// PostgreSQL startup response. A backend startup pointed at this listener
/// stays in login until pg_doorman's `connect_timeout` cancels pool checkout.
///
/// The helper stores the selected port in `world.vars` as `<NAME>_PORT` for
/// `${<NAME>_PORT}` config expansion.
#[given(regex = r"^TCP blackhole listener registered as '(.+)'$")]
pub async fn start_blackhole_listener(world: &mut DoormanWorld, name: String) {
    let port = portpicker::pick_unused_port().expect("no free ports");
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .expect("failed to bind blackhole listener");

    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(async move {
                        let _hold = stream;
                        futures::future::pending::<()>().await
                    });
                }
                Err(_) => break,
            }
        }
    });
    world.blackhole_aborts.0.push(handle);

    let var_name = format!("{}_PORT", name.to_uppercase());
    world.vars.insert(var_name, port.to_string());
}

/// Starts a client session and ignores the expected startup failure. Scenarios
/// use this step when the assertion belongs to pg_doorman's visible state
/// (`SHOW SERVERS`, logs), not to the client-side error message.
///
/// The connection runs in a task because the protocol helpers may panic after
/// pg_doorman closes the half-open startup. The join result is ignored with
/// the startup error; later steps assert on the pooler state.
#[when(
    regex = r#"^we attempt session to pg_doorman as "([^"]+)" with password "([^"]*)" and database "([^"]+)" expecting startup failure$"#
)]
pub async fn attempt_session_expecting_startup_failure(
    world: &mut DoormanWorld,
    user: String,
    password: String,
    database: String,
) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

    let handle = tokio::spawn(async move {
        let mut conn = PgConnection::connect(&doorman_addr).await.ok()?;
        conn.send_startup(&user, &database).await.ok()?;
        let _ = conn.authenticate(&user, &password).await;
        Some(())
    });
    match handle.await {
        Ok(_) => {}
        Err(join_err) if join_err.is_panic() => {
            // The PgConnection helper panics when pg_doorman replies with a
            // FATAL ErrorResponse; the scenario expects that path.
        }
        Err(join_err) => {
            panic!("blackhole attempt task aborted unexpectedly: {join_err:?}");
        }
    }
}

/// Polls `SHOW SERVERS` on the named admin session until the row count
/// matches the expected value or the deadline expires. Used by scenarios
/// that drop a `ServerStatsRegistration` asynchronously and need a
/// deterministic wait without padding `sleep` for the slowest CI host.
#[then(
    regex = r#"^we poll "SHOW SERVERS" on admin session "([^"]+)" until row count is (\d+) within (\d+)ms$"#
)]
pub async fn poll_show_servers_until_row_count(
    world: &mut DoormanWorld,
    session_name: String,
    expected_rows: usize,
    deadline_ms: u64,
) {
    let conn = crate::extended::helpers::get_session(&mut world.named_sessions, &session_name);
    let deadline = Instant::now() + Duration::from_millis(deadline_ms);
    let poll_interval = Duration::from_millis(50);

    loop {
        conn.send_simple_query("SHOW SERVERS")
            .await
            .expect("failed to send SHOW SERVERS");
        let mut rows = 0usize;
        loop {
            let (msg_type, data) = conn
                .read_message()
                .await
                .expect("failed to read SHOW SERVERS response");
            match msg_type {
                'D' => rows += 1,
                'Z' => break,
                'E' => panic!(
                    "admin session '{}' returned error during SHOW SERVERS poll: {:?}",
                    session_name,
                    String::from_utf8_lossy(&data)
                ),
                _ => {}
            }
        }
        if rows == expected_rows {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "SHOW SERVERS still reports {rows} rows on admin session '{session_name}' after {deadline_ms}ms; expected {expected_rows}"
            );
        }
        tokio::time::sleep(poll_interval).await;
    }
}
