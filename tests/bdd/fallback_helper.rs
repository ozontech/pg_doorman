//! BDD helpers for the fallback path.
//!
//! Two pieces:
//! - `we start hung TCP listener as '{name}'`: binds a random free port and
//!   accepts inbound TCP, but never replies. Used to simulate a postgres
//!   that opened its TCP listener but never gets past StartupMessage —
//!   exercises `startup_with_timeout` and per-candidate cooldown without
//!   needing a second real PostgreSQL.
//! - `psql connection ... fails with error containing {string}`: TCP variant
//!   of the existing unix-socket error-message matcher; lets scenarios
//!   assert the exact `"all fallback candidates rejected (...)"` text that
//!   `create_fallback_connection` returns on exhaustion.
use crate::postgres_helper::build_psql_via_doorman;
use crate::world::DoormanWorld;
use cucumber::{given, then};
use std::process::Stdio;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::sleep;

/// Bind a TCP listener that accepts connections and immediately parks them
/// without writing a single byte. The chosen port is exposed as
/// `${HUNG_<NAME>_PORT}` for substitution in subsequent feature steps
/// (most usefully inside the Patroni mock JSON members list).
///
/// Lifetime: the listener task lives until the tokio runtime shuts down at
/// scenario end. No explicit cleanup — there is nothing to leak across
/// scenarios because the runtime is recreated per scenario.
#[given(regex = r"^we start hung TCP listener as '(.+)'$")]
pub async fn start_hung_tcp_listener(world: &mut DoormanWorld, name: String) {
    let port = portpicker::pick_unused_port().expect("no free ports");
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .expect("failed to bind hung TCP listener");
    tokio::spawn(async move {
        // Accept loop: hold each accepted stream for an hour so the kernel
        // keeps the connection in ESTABLISHED state but the client side
        // never sees a single byte back. One hour is much longer than any
        // reasonable scenario `-T`, so we never close prematurely.
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let _hold = stream;
                sleep(Duration::from_secs(3600)).await;
            });
        }
    });
    world.vars.insert(
        format!("HUNG_{}_PORT", name.to_uppercase()),
        port.to_string(),
    );
}

/// TCP connection variant of `psql connection ... fails with`. Captures
/// stderr from `psql` and asserts the requested substring is present.
/// Useful for asserting the wording of pg_doorman exhaustion errors that
/// surface to the client as PostgreSQL FATAL messages.
#[then(
    expr = "psql connection to pg_doorman as user {string} to database {string} with password {string} fails with error containing {string}"
)]
pub async fn psql_connection_fails_with_error(
    world: &mut DoormanWorld,
    user: String,
    database: String,
    password: String,
    needle: String,
) {
    let port = world.doorman_port.expect("pg_doorman not started");
    let output = build_psql_via_doorman(port, &user, &database, "SELECT 1", Some(&password))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to run psql");
    assert!(
        !output.status.success(),
        "psql connection unexpectedly succeeded (user: {user}, database: {database})"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&needle),
        "psql stderr did not contain {needle:?}. Full stderr:\n{stderr}"
    );
}
