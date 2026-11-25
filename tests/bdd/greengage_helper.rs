//! Opt-in BDD fixture for the real, externally managed Greengage cluster.
//! Each scenario owns one database; the Docker service owns the cluster.

use crate::extended::helpers::{parse_datarow_fields, ProtocolMessages};
use crate::pg_connection::PgConnection;
use crate::world::DoormanWorld;
use cucumber::{given, then};
use std::time::Duration;
use tokio::time::{interval, timeout};

pub struct GreengageDatabase {
    database: String,
    admin: PgConnection,
}

async fn connect(addr: &str, user: &str, password: &str, database: &str) -> PgConnection {
    timeout(Duration::from_secs(15), async {
        let mut connection = PgConnection::connect(addr)
            .await
            .expect("Failed to connect to the Greengage coordinator");
        connection
            .send_startup(user, database)
            .await
            .expect("Failed to send Greengage startup");
        connection
            .authenticate(user, password)
            .await
            .expect("Failed to authenticate to Greengage");
        connection
    })
    .await
    .expect("Timed out connecting to Greengage")
}

async fn query(connection: &mut PgConnection, sql: &str) -> ProtocolMessages {
    let messages = timeout(Duration::from_secs(15), async {
        connection.send_simple_query(sql).await?;
        connection.read_all_messages_until_ready().await
    })
    .await
    .unwrap_or_else(|_| panic!("Greengage fixture query timed out: {sql}"))
    .unwrap_or_else(|error| panic!("Greengage fixture query failed: {sql}: {error}"));
    for (tag, body) in &messages {
        assert_ne!(
            *tag,
            'E',
            "Greengage fixture SQL failed: {sql}: {}",
            String::from_utf8_lossy(body)
        );
    }
    messages
}

fn first_row(messages: &ProtocolMessages) -> Vec<String> {
    messages
        .iter()
        .find(|(tag, _)| *tag == 'D')
        .map(|(_, body)| parse_datarow_fields(body))
        .expect("Greengage fixture query returned no row")
}

#[given("a temporary Greengage database on the configured cluster")]
pub async fn create_database(world: &mut DoormanWorld) {
    let host = std::env::var("GREENGAGE_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("GREENGAGE_PORT")
        .expect("Set GREENGAGE_PORT to run @greengage scenarios")
        .parse::<u16>()
        .expect("GREENGAGE_PORT must be a TCP port");
    let user = std::env::var("GREENGAGE_USER").unwrap_or_else(|_| "gpadmin".into());
    let password = std::env::var("GREENGAGE_PASSWORD").unwrap_or_else(|_| "greengage-test".into());
    let addr = format!("{host}:{port}");
    let mut admin = connect(&addr, &user, &password, "postgres").await;
    let version = first_row(&query(&mut admin, "SELECT version()").await).remove(0);
    assert!(
        version.contains("Greengage"),
        "@greengage requires a real Greengage server, got: {version}"
    );
    let segments = first_row(
        &query(
            &mut admin,
            "SELECT count(*) FROM gp_segment_configuration WHERE content >= 0 AND role = 'p' AND status = 'u'",
        )
        .await,
    )
    .remove(0);
    assert!(
        segments.parse::<usize>().expect("Invalid segment count") >= 2,
        "Greengage BDD requires at least two running primary segments"
    );

    // Generated identifiers contain only ASCII letters, digits and underscores.
    let database = format!(
        "doorman_bdd_{}_{:016x}",
        std::process::id(),
        rand::random::<u64>()
    );
    world.greengage_database = Some(GreengageDatabase {
        database: database.clone(),
        admin,
    });
    query(
        &mut world.greengage_database.as_mut().unwrap().admin,
        &format!("CREATE DATABASE {database}"),
    )
    .await;
    for (name, value) in [
        ("GREENGAGE_HOST", host),
        ("GREENGAGE_PORT", port.to_string()),
        ("GREENGAGE_DATABASE", database.clone()),
        ("GREENGAGE_USER", user.clone()),
        ("GREENGAGE_PASSWORD", password.clone()),
        ("GREENGAGE_SEGMENTS", segments),
    ] {
        world.vars.insert(name.into(), value);
    }
    let observer = connect(&addr, &user, &password, &database).await;
    world.named_sessions.insert("observer".into(), observer);
    println!("Greengage fixture: {version}; database {database}");
}

/// Called after pg_doorman is stopped, including when a scenario failed.
pub async fn drop_database(world: &mut DoormanWorld) {
    let Some(mut fixture) = world.greengage_database.take() else {
        return;
    };
    world.named_sessions.clear();
    world.pg_conn = None;
    world.doorman_conn = None;
    timeout(Duration::from_secs(20), async {
        let mut poll = interval(Duration::from_millis(50));
        loop {
            query(
                &mut fixture.admin,
                &format!(
                    "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}' AND pid <> pg_backend_pid()",
                    fixture.database
                ),
            )
            .await;
            let active = first_row(
                &query(
                    &mut fixture.admin,
                    &format!(
                        "SELECT count(*) FROM pg_stat_activity WHERE datname = '{}'",
                        fixture.database
                    ),
                )
                .await,
            );
            if active[0] == "0" {
                break;
            }
            poll.tick().await;
        }
        query(
            &mut fixture.admin,
            &format!("DROP DATABASE IF EXISTS {}", fixture.database),
        )
        .await;
    })
    .await
    .expect("Timed out removing the scenario's Greengage database");
}

fn sqlstate(body: &[u8]) -> Option<&str> {
    let mut fields = body.split(|byte| *byte == 0);
    fields.find_map(|field| {
        (field.first() == Some(&b'C'))
            .then(|| std::str::from_utf8(&field[1..]).ok())
            .flatten()
    })
}

#[then(regex = r#"^session "([^"]+)" should receive NoticeResponse with SQLSTATE "([^"]+)"$"#)]
pub async fn notice_with_sqlstate(world: &mut DoormanWorld, session: String, expected: String) {
    let messages = &world.session_messages[&session];
    assert!(
        messages
            .iter()
            .any(|(tag, body)| *tag == 'N' && sqlstate(body) == Some(expected.as_str())),
        "Session {session}: expected NoticeResponse with SQLSTATE {expected}, got {messages:?}"
    );
}

#[then(regex = r#"^session "([^"]+)" should receive only backend messages "([A-Za-z0-9]+)"$"#)]
pub async fn exact_message_tags(world: &mut DoormanWorld, session: String, expected: String) {
    let messages = &world.session_messages[&session];
    let actual: String = messages.iter().map(|(tag, _)| *tag).collect();
    assert_eq!(
        actual, expected,
        "Session {session}: unexpected protocol messages: {messages:?}"
    );
}
