use crate::extended::helpers::{format_message_details, get_session, parse_datarow_fields};
use crate::pg_connection::PgConnection;
use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, then, when};
use serde::{Deserialize, Serialize};
use std::io;
use std::time::{Duration, Instant};
use tokio::time::timeout;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchSpec {
    queries: usize,
    delay_ms: u64,
    row_bytes: usize,
    padding_bytes: usize,
    timeout_ms: u64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BatchReport {
    rows: usize,
    command_completes: usize,
    first_row_ms: Option<u64>,
    last_row_ms: Option<u64>,
    elapsed_ms: u64,
    ready: bool,
    error: Option<String>,
}

fn report(world: &DoormanWorld, session: &str) -> BatchReport {
    let json = world
        .vars
        .get(&format!("batch_report:{session}"))
        .expect("no batch report for this session");
    serde_json::from_str(json).expect("invalid batch report")
}

async fn send_batch(conn: &mut PgConnection, spec: &BatchSpec) -> io::Result<()> {
    let padding = "x".repeat(spec.padding_bytes);
    for index in 0..spec.queries {
        let sql = format!(
            "SELECT {index}, repeat('x', {}), pg_sleep({}.{:03}) /* {padding} */",
            spec.row_bytes,
            spec.delay_ms / 1000,
            spec.delay_ms % 1000,
        );
        // P/B/D/E for every command, followed by a single Sync for the batch.
        conn.send_parse("", &sql).await?;
        conn.send_bind("", "", vec![]).await?;
        conn.send_describe('P', "").await?;
        conn.send_execute("", 0).await?;
    }
    conn.send_sync().await
}

async fn read_batch(
    conn: &mut PgConnection,
    spec: &BatchSpec,
    started: Instant,
    report: &mut BatchReport,
) -> io::Result<()> {
    loop {
        let (kind, body) = conn.read_message().await?;
        match kind {
            'D' => {
                let fields = parse_datarow_fields(&body);
                if fields.len() != 3
                    || fields[0] != report.rows.to_string()
                    || fields[1].len() != spec.row_bytes
                    || !fields[1].bytes().all(|byte| byte == b'x')
                {
                    return Err(io::Error::other(format!(
                        "invalid or out-of-order batch row {}",
                        report.rows
                    )));
                }
                let elapsed = started.elapsed().as_millis() as u64;
                report.rows += 1;
                report.first_row_ms.get_or_insert(elapsed);
                report.last_row_ms = Some(elapsed);
            }
            'C' => report.command_completes += 1,
            'E' => {
                report.error = Some(format_message_details(kind, &body));
                return Ok(());
            }
            'Z' => {
                if body != [b'I'] {
                    return Err(io::Error::other("batch did not finish in idle state"));
                }
                report.ready = true;
                return Ok(());
            }
            _ => {}
        }
    }
}

#[when(regex = r#"^we exchange the following extended batch on session "([^"]+)":$"#)]
pub async fn exchange_batch(world: &mut DoormanWorld, session: String, step: &Step) {
    let table = step.table.as_ref().expect("batch parameters table missing");
    let mut parameters = serde_json::Map::new();
    for row in &table.rows {
        assert_eq!(row.len(), 2, "expected parameter name and numeric value");
        let value: u64 = row[1].parse().expect("batch parameter must be an integer");
        assert!(
            parameters
                .insert(row[0].clone(), serde_json::json!(value))
                .is_none(),
            "duplicate batch parameter"
        );
    }
    let spec: BatchSpec =
        serde_json::from_value(parameters.into()).expect("invalid batch parameters");
    assert!(spec.queries > 0 && spec.timeout_ms > 0);
    let conn = get_session(&mut world.named_sessions, &session);
    let started = Instant::now();
    let mut result = BatchReport::default();
    // The client drains replies immediately. This test does not simulate a
    // slow client or freeze PostgreSQL; pg_sleep only paces command execution.
    let outcome = timeout(Duration::from_millis(spec.timeout_ms), async {
        send_batch(conn, &spec).await?;
        read_batch(conn, &spec, started, &mut result).await
    })
    .await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(err)) => result.error = Some(format!("batch I/O failed: {err}")),
        Err(_) => result.error = Some(format!("test deadline exceeded ({}ms)", spec.timeout_ms)),
    }
    result.elapsed_ms = started.elapsed().as_millis() as u64;
    eprintln!("Batch on session {session:?}: {result:?}");
    world.vars.insert(
        format!("batch_report:{session}"),
        serde_json::to_string(&result).expect("failed to serialize batch report"),
    );
}

#[then(regex = r#"^session "([^"]+)" should receive batch rows over at least (\d+)ms$"#)]
pub async fn batch_makes_progress(world: &mut DoormanWorld, session: String, span_ms: u64) {
    let result = report(world, &session);
    let span = result
        .first_row_ms
        .zip(result.last_row_ms)
        .map(|(first, last)| last.saturating_sub(first));
    assert!(
        result.rows >= 2 && span.is_some_and(|span| span >= span_ms),
        "expected replies arriving over at least {span_ms}ms: {result:?}"
    );
}

#[then(regex = r#"^session "([^"]+)" should complete all (\d+) batch queries without error$"#)]
pub async fn batch_completes(world: &mut DoormanWorld, session: String, expected: usize) {
    let result = report(world, &session);
    assert!(
        result.error.is_none()
            && result.ready
            && result.rows == expected
            && result.command_completes == expected,
        "expected {expected} ordered rows, CommandComplete for every query and idle ReadyForQuery: {result:?}"
    );
}

#[then(regex = r#"^session "([^"]+)" should accept another query after the batch$"#)]
pub async fn batch_session_remains_usable(world: &mut DoormanWorld, session: String) {
    let conn = get_session(&mut world.named_sessions, &session);
    timeout(Duration::from_secs(5), async {
        conn.send_simple_query("SELECT 42").await.unwrap();
        let messages = conn.read_all_messages_until_ready().await.unwrap();
        assert!(!messages.iter().any(|(kind, _)| *kind == 'E'));
        assert!(messages
            .iter()
            .any(|(kind, body)| { *kind == 'D' && parse_datarow_fields(body) == ["42"] }));
    })
    .await
    .expect("session stopped responding after a completed batch");
}
