//! Wire-level coverage of Parse novelty and live RELOAD on an existing client.

use crate::{pg_connection::PgConnection, world::DoormanWorld};
use cucumber::{gherkin::Step, given, then};
use std::time::Duration;

/// An optional externally provisioned, trust-authenticated PostgreSQL avoids
/// requiring server binaries on machines that only have libpq installed.
/// The caller owns its lifecycle; the default uses the regular BDD fixture.
#[given("PostgreSQL for adhoc tracking started with pg_hba.conf:")]
async fn postgres(world: &mut DoormanWorld, step: &Step) {
    if let Ok(port) = std::env::var("PG_DOORMAN_ADHOC_TEST_PG_PORT") {
        world.pg_port = Some(port.parse().expect("invalid external PostgreSQL port"));
    } else {
        crate::postgres_helper::start_postgres_with_hba(world, step).await;
    }
}

#[given("adhoc metrics listener uses a free port")]
fn metrics_port(world: &mut DoormanWorld) {
    world.vars.insert(
        "ADHOC_METRICS_PORT".into(),
        portpicker::pick_unused_port().unwrap().to_string(),
    );
}

async fn connect(world: &DoormanWorld, admin: bool) -> PgConnection {
    let mut conn = PgConnection::connect(&format!("127.0.0.1:{}", world.doorman_port.unwrap()))
        .await
        .unwrap();
    let (user, database, password) = if admin {
        ("admin", "pgbouncer", "admin")
    } else {
        ("postgres", "postgres", "")
    };
    conn.send_startup(user, database).await.unwrap();
    conn.authenticate(user, password).await.unwrap();
    conn
}

async fn ready(conn: &mut PgConnection, expect_error: bool) -> Vec<(char, Vec<u8>)> {
    let messages =
        tokio::time::timeout(Duration::from_secs(5), conn.read_all_messages_until_ready())
            .await
            .expect("protocol response timed out")
            .unwrap();
    assert_eq!(
        messages.iter().any(|(kind, _)| *kind == 'E'),
        expect_error,
        "unexpected protocol response: {messages:?}"
    );
    messages
}

async fn parsed_query(conn: &mut PgConnection, name: &str, query: &str) {
    conn.send_parse(name, query).await.unwrap();
    bound_query(conn, name).await;
}

async fn bound_query(conn: &mut PgConnection, name: &str) {
    conn.send_bind("", name, Vec::new()).await.unwrap();
    conn.send_execute("", 0).await.unwrap();
    conn.send_sync().await.unwrap();
    ready(conn, false).await;
}

async fn backend_pid(conn: &mut PgConnection) -> i32 {
    conn.send_simple_query("SELECT pg_backend_pid()")
        .await
        .unwrap();
    let messages = ready(conn, false).await;
    let row = &messages.iter().find(|(kind, _)| *kind == 'D').unwrap().1;
    assert_eq!(u16::from_be_bytes(row[..2].try_into().unwrap()), 1);
    let len = i32::from_be_bytes(row[2..6].try_into().unwrap()) as usize;
    std::str::from_utf8(&row[6..6 + len])
        .unwrap()
        .parse()
        .unwrap()
}

async fn assert_counts(world: &DoormanWorld, observed: u64, candidates: u64) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let text = client
        .get(format!(
            "http://127.0.0.1:{}/metrics",
            world.vars["ADHOC_METRICS_PORT"]
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    for (metric, expected) in [
        ("pg_doorman_adhoc_observed_parses_total", observed),
        ("pg_doorman_adhoc_candidate_parses_total", candidates),
        ("pg_doorman_adhoc_filter_resets_total", 0),
    ] {
        let value = text.lines().find_map(|line| {
            if line.starts_with(&format!("{metric}{{"))
                && line.contains("user=\"postgres\"")
                && line.contains("database=\"postgres\"")
            {
                Some(line.rsplit_once(' ').unwrap().1.parse::<u64>().unwrap())
            } else {
                None
            }
        });
        assert_eq!(value.unwrap_or(0), expected, "{metric}:\n{text}");
    }
}

async fn reload(world: &DoormanWorld, admin: &mut PgConnection, enabled: bool) {
    let path = world.doorman_config_file.as_ref().unwrap().path();
    let mut config: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    config["general"]["adhoc_query_tracking"] = enabled.into();
    std::fs::write(path, serde_yaml::to_string(&config).unwrap()).unwrap();
    admin.send_simple_query("RELOAD").await.unwrap();
    ready(admin, false).await;
}

#[then("adhoc tracking follows Parse novelty and live reload without reconnecting")]
async fn novelty_and_reload(world: &mut DoormanWorld) {
    let mut data = connect(world, false).await;
    let mut admin = connect(world, true).await;
    let pid = backend_pid(&mut data).await;

    // Disabled at startup, even though this client uses the extended protocol.
    parsed_query(&mut data, "disabled", "SELECT 101").await;
    assert_counts(world, 0, 0).await;

    reload(world, &mut admin, true).await;
    parsed_query(&mut data, "first", "SELECT 101").await;
    parsed_query(&mut data, "different_name", "SELECT 101").await;
    assert_counts(world, 2, 1).await;

    // Bind/Execute without Parse and Simple Query do not affect either counter.
    bound_query(&mut data, "first").await;
    data.send_simple_query("SELECT 999").await.unwrap();
    ready(&mut data, false).await;
    assert_counts(world, 2, 1).await;

    // Another client shares history, and disconnecting does not erase counts.
    let mut other = connect(world, false).await;
    parsed_query(&mut other, "other_client", "SELECT 101").await;
    drop(other);
    assert_counts(world, 3, 1).await;

    reload(world, &mut admin, true).await;
    parsed_query(&mut data, "unchanged_reload", "SELECT 101").await;
    assert_counts(world, 4, 1).await;

    // A failed reload that requested disabling must leave tracking enabled.
    let path = world.doorman_config_file.as_ref().unwrap().path();
    let valid = std::fs::read_to_string(path).unwrap();
    let mut invalid: serde_yaml::Value = serde_yaml::from_str(&valid).unwrap();
    invalid["general"]["adhoc_query_tracking"] = false.into();
    invalid["general"]["port"] = "invalid-port".into();
    std::fs::write(path, serde_yaml::to_string(&invalid).unwrap()).unwrap();
    admin.send_simple_query("RELOAD").await.unwrap();
    // The existing admin protocol may close the admin socket on a config
    // error rather than sending ReadyForQuery. The data client stays open.
    let rejected = tokio::time::timeout(Duration::from_secs(5), admin.read_message())
        .await
        .expect("failed reload did not respond or close its admin connection");
    assert!(
        rejected.is_err() || matches!(rejected, Ok(('E', _))),
        "invalid config reload unexpectedly succeeded"
    );
    std::fs::write(path, valid).unwrap();
    parsed_query(&mut data, "failed_reload", "SELECT 103").await;
    assert_counts(world, 5, 2).await;

    admin = connect(world, true).await;
    reload(world, &mut admin, false).await;
    parsed_query(&mut data, "disabled_again", "SELECT 104").await;
    assert_counts(world, 5, 2).await;
    reload(world, &mut admin, true).await;
    parsed_query(&mut data, "fresh_history", "SELECT 101").await;
    assert_counts(world, 6, 3).await;
    assert_eq!(backend_pid(&mut data).await, pid);
}

#[then("adhoc tracking hashes raw Parse SQL when prepared statements are disabled")]
async fn passthrough_tracking(world: &mut DoormanWorld) {
    let mut data = connect(world, false).await;
    let mut admin = connect(world, true).await;
    let pid = backend_pid(&mut data).await;

    // The SQL C string is unterminated, but the outer frame is complete.
    // Tracking must let PostgreSQL reject it rather than aborting the client.
    data.send_raw_message(b'P', b"broken\0SELECT 1")
        .await
        .unwrap();
    data.send_sync().await.unwrap();
    let messages = ready(&mut data, true).await;
    assert!(messages
        .iter()
        .any(|(kind, body)| { *kind == 'E' && body.windows(7).any(|field| field == b"C08P01\0") }));
    assert_counts(world, 0, 0).await;
    assert_eq!(backend_pid(&mut data).await, pid);

    parsed_query(&mut data, "passthrough", "SELECT 101").await;
    assert_counts(world, 1, 1).await;
    parsed_query(&mut data, "different_name", "SELECT 101").await;
    parsed_query(&mut data, "", "SELECT 101").await;
    assert_counts(world, 3, 1).await;
    parsed_query(&mut data, "different_sql", "SELECT 202").await;
    assert_counts(world, 4, 2).await;

    data.send_simple_query("SELECT 999").await.unwrap();
    ready(&mut data, false).await;
    assert_counts(world, 4, 2).await;

    reload(world, &mut admin, false).await;
    parsed_query(&mut data, "tracking_disabled", "SELECT 303").await;
    assert_counts(world, 4, 2).await;
    reload(world, &mut admin, true).await;
    parsed_query(&mut data, "tracking_enabled", "SELECT 101").await;
    assert_counts(world, 5, 3).await;
    assert_eq!(backend_pid(&mut data).await, pid);
}
