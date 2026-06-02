use crate::pg_connection::PgConnection;
use crate::world::DoormanWorld;
use cucumber::{then, when};
use log::info;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::time::timeout;

/// Count recorded by `attempt_create_idle_sessions`.
const ACCEPTED_VAR_KEY: &str = "last_idle_attempt_accepted";

/// Concurrent best-effort session creation for fd-pressure scenarios.
/// Some attempts may fail; successes are stored as `idle-N`.
#[when(
    regex = r#"^we attempt to create (\d+) idle sessions to pg_doorman as "([^"]+)" with password "([^"]*)" and database "([^"]+)"$"#
)]
pub async fn attempt_create_idle_sessions(
    world: &mut DoormanWorld,
    count: usize,
    user: String,
    password: String,
    database: String,
) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

    let step_timeout = Duration::from_millis(2000);

    // Protocol FATAL can panic inside helper auth; JoinSet counts it
    // instead of aborting the scenario.
    let mut set: JoinSet<Result<(usize, PgConnection), &'static str>> = JoinSet::new();
    for idx in 0..count {
        let user = user.clone();
        let password = password.clone();
        let database = database.clone();
        let doorman_addr = doorman_addr.clone();
        set.spawn(async move {
            let mut conn = match timeout(step_timeout, PgConnection::connect(&doorman_addr)).await {
                Ok(Ok(c)) => c,
                Ok(Err(_)) => return Err("connect_err"),
                Err(_) => return Err("connect_timeout"),
            };
            match timeout(step_timeout, conn.send_startup(&user, &database)).await {
                Ok(Ok(())) => {}
                _ => return Err("startup_failed"),
            }
            match timeout(step_timeout, conn.authenticate(&user, &password)).await {
                Ok(Ok(())) => {}
                _ => return Err("authenticate_failed"),
            }
            Ok((idx, conn))
        });
    }

    let mut accepted = 0usize;
    let mut timed_out = 0usize;
    let mut connect_err = 0usize;
    let mut startup_failed = 0usize;
    let mut authenticate_failed = 0usize;
    let mut panicked = 0usize;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok((idx, conn))) => {
                world.named_sessions.insert(format!("idle-{idx}"), conn);
                accepted += 1;
            }
            Ok(Err("connect_timeout")) => timed_out += 1,
            Ok(Err("connect_err")) => connect_err += 1,
            Ok(Err("startup_failed")) => startup_failed += 1,
            Ok(Err("authenticate_failed")) => authenticate_failed += 1,
            Ok(Err(_)) => {}
            Err(_) => panicked += 1,
        }
    }

    world
        .vars
        .insert(ACCEPTED_VAR_KEY.to_string(), accepted.to_string());
    info!(
        "attempt_create_idle_sessions: requested {count}, accepted {accepted}, \
         connect_err {connect_err}, connect_timeout {timed_out}, \
         startup_failed {startup_failed}, authenticate_failed {authenticate_failed}, \
         panicked {panicked}"
    );
}

/// Require retained idle clients so fd checks cannot pass with zero sessions.
#[then(regex = r#"^at least (\d+) idle sessions should be open from the last batch attempt$"#)]
pub async fn at_least_n_idle_sessions_open(world: &mut DoormanWorld, min: usize) {
    let accepted: usize = world
        .vars
        .get(ACCEPTED_VAR_KEY)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    assert!(
        accepted >= min,
        "expected at least {min} accepted sessions from the last batch attempt, got {accepted}"
    );
}

/// Verify the new process can complete PostgreSQL startup/auth after upgrade.
/// Retry because listener readiness can precede migration drain.
#[then(
    regex = r#"^a fresh PostgreSQL session to pg_doorman as "([^"]+)" with password "([^"]*)" and database "([^"]+)" succeeds$"#
)]
pub async fn fresh_pg_session_succeeds(
    world: &mut DoormanWorld,
    user: String,
    password: String,
    database: String,
) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);
    let attempt_timeout = Duration::from_secs(3);
    let overall_budget = Duration::from_secs(20);
    let retry_delay = Duration::from_millis(250);
    let deadline = std::time::Instant::now() + overall_budget;

    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let outcome = async {
            let mut conn = timeout(attempt_timeout, PgConnection::connect(&doorman_addr))
                .await
                .map_err(|_| "connect timed out".to_string())?
                .map_err(|e| format!("connect failed: {e}"))?;
            timeout(attempt_timeout, conn.send_startup(&user, &database))
                .await
                .map_err(|_| "send_startup timed out".to_string())?
                .map_err(|e| format!("send_startup failed: {e}"))?;
            timeout(attempt_timeout, conn.authenticate(&user, &password))
                .await
                .map_err(|_| "authenticate timed out".to_string())?
                .map_err(|e| format!("authenticate failed: {e}"))?;
            Ok::<(), String>(())
        }
        .await;

        match outcome {
            Ok(()) => return,
            Err(reason) if std::time::Instant::now() >= deadline => {
                panic!(
                    "fresh session: gave up after {attempt} attempts ({}ms budget): {reason}",
                    overall_budget.as_millis()
                );
            }
            Err(_) => {
                tokio::time::sleep(retry_delay).await;
            }
        }
    }
}

/// Round-trip every retained `idle-N` session and store the success count.
/// Prevents a socket-count pass after dropping migrated clients.
#[when(
    regex = r#"^we send SimpleQuery "([^"]+)" to every open idle session and count successes as "([^"]+)"$"#
)]
pub async fn send_query_to_every_idle_session(
    world: &mut DoormanWorld,
    query: String,
    out_count_key: String,
) {
    use std::time::Duration;
    use tokio::time::timeout;

    let names: Vec<String> = world
        .named_sessions
        .keys()
        .filter(|k| k.starts_with("idle-"))
        .cloned()
        .collect();
    let mut sorted = names;
    sorted.sort_by_key(|k| {
        k.strip_prefix("idle-")
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(usize::MAX)
    });

    let step_timeout = Duration::from_secs(3);
    let mut successes: usize = 0;
    let mut failures: Vec<(String, String)> = Vec::new();
    for name in sorted {
        // Missing sessions count as continuity failures, not harness panics.
        let Some(conn) = world.named_sessions.get_mut(&name) else {
            failures.push((name.clone(), "session missing".into()));
            continue;
        };
        let send = timeout(step_timeout, conn.send_simple_query(&query)).await;
        match send {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                failures.push((name.clone(), format!("send_simple_query: {e}")));
                world.named_sessions.remove(&name);
                continue;
            }
            Err(_) => {
                failures.push((name.clone(), "send_simple_query timed out".into()));
                world.named_sessions.remove(&name);
                continue;
            }
        }
        let read = timeout(step_timeout, conn.read_all_messages_until_ready()).await;
        let messages = match read {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => {
                failures.push((name.clone(), format!("read response: {e}")));
                world.named_sessions.remove(&name);
                continue;
            }
            Err(_) => {
                failures.push((name.clone(), "read response timed out".into()));
                world.named_sessions.remove(&name);
                continue;
            }
        };
        let mut found_row = false;
        for (msg_type, data) in &messages {
            if *msg_type == 'D' {
                let fields = super::helpers::parse_datarow_fields(data);
                if fields.into_iter().next().is_some() {
                    found_row = true;
                    break;
                }
            } else if *msg_type == 'E' {
                failures.push((
                    name.clone(),
                    format!("ErrorResponse: {}", String::from_utf8_lossy(data)),
                ));
                break;
            }
        }
        if found_row {
            successes += 1;
        } else {
            failures.push((name.clone(), "no DataRow in response".into()));
            world.named_sessions.remove(&name);
        }
    }
    log::info!(
        "[binary-upgrade-fd] round-tripped {} session(s), {} succeeded, {} failed",
        successes + failures.len(),
        successes,
        failures.len()
    );
    if !failures.is_empty() {
        let preview: Vec<String> = failures
            .iter()
            .take(10)
            .map(|(s, e)| format!("{s}: {e}"))
            .collect();
        log::info!(
            "[binary-upgrade-fd] failure preview: {}",
            preview.join("; ")
        );
    }
    world.vars.insert(out_count_key, successes.to_string());
}

#[then(regex = r#"^the stored count "([^"]+)" should be at least (\d+)$"#)]
pub async fn stored_count_at_least(world: &mut DoormanWorld, key: String, min: usize) {
    let raw = world.vars.get(&key).unwrap_or_else(|| {
        panic!(
            "no count stored under key '{key}' — capture it via a prior `count successes as` step"
        )
    });
    let actual: usize = raw
        .parse()
        .unwrap_or_else(|e| panic!("count '{key}' = {raw:?} is not numeric: {e}"));
    assert!(
        actual >= min,
        "expected at least {min} successes under '{key}', got {actual}"
    );
}

/// Run a SimpleQuery on the first retained `idle-N` session.
#[when(
    regex = r#"^we send SimpleQuery "([^"]+)" to the first open idle session and store response as "([^"]+)"$"#
)]
pub async fn send_query_to_first_idle_session(
    world: &mut DoormanWorld,
    query: String,
    out_session_name: String,
) {
    // The batch step inserts under "idle-{idx}" with idx ∈ [0, count).
    // We don't know which one survived, so scan in idx order and use
    // the first key that exists. This is deterministic across runs
    // because indices are monotonic, but tolerant to indices missing
    // (rejected attempts left a hole at that idx).
    let name = world
        .named_sessions
        .keys()
        .filter(|k| k.starts_with("idle-"))
        .min_by_key(|k| {
            k.strip_prefix("idle-")
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(usize::MAX)
        })
        .cloned()
        .expect("no `idle-N` session available — batch step rejected all attempts");

    let conn = super::helpers::get_session(&mut world.named_sessions, &name);
    conn.send_simple_query(&query)
        .await
        .expect("Failed to send SimpleQuery to first idle session");
    let messages = conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read SimpleQuery response from first idle session");
    world.session_messages.insert(out_session_name, messages);
}

#[when(
    regex = r#"^we create session "([^"]+)" to pg_doorman as "([^"]+)" with password "([^"]*)" and database "([^"]+)"$"#
)]
pub async fn create_named_session(
    world: &mut DoormanWorld,
    session_name: String,
    user: String,
    password: String,
    database: String,
) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

    // Connect to pg_doorman
    let mut conn = PgConnection::connect(&doorman_addr)
        .await
        .expect("Failed to connect to pg_doorman");
    conn.send_startup(&user, &database)
        .await
        .expect("Failed to send startup to pg_doorman");
    conn.authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to pg_doorman");

    world.named_sessions.insert(session_name, conn);
}

/// Create a session with extra StartupMessage parameters.
/// `extras` is a comma-separated list of `key=value` pairs.
#[when(
    regex = r#"^we create session "([^"]+)" to pg_doorman as "([^"]+)" with password "([^"]*)" and database "([^"]+)" and startup parameters "([^"]+)"$"#
)]
pub async fn create_named_session_with_startup_params(
    world: &mut DoormanWorld,
    session_name: String,
    user: String,
    password: String,
    database: String,
    extras: String,
) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

    let parsed: Vec<(String, String)> = extras
        .split(',')
        .filter_map(|entry| {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                return None;
            }
            let (k, v) = trimmed.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    let extras_ref: Vec<(&str, &str)> = parsed
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut conn = PgConnection::connect(&doorman_addr)
        .await
        .expect("Failed to connect to pg_doorman");
    conn.send_startup_with_params(&user, &database, &extras_ref)
        .await
        .expect("Failed to send startup with params to pg_doorman");
    conn.authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to pg_doorman");

    world.named_sessions.insert(session_name, conn);
}

#[when(
    regex = r#"^we create (\d+) sessions with prefix "([^"]+)" to pg_doorman as "([^"]+)" with password "([^"]*)" and database "([^"]+)"$"#
)]
pub async fn create_sessions_with_prefix(
    world: &mut DoormanWorld,
    count: usize,
    prefix: String,
    user: String,
    password: String,
    database: String,
) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

    for idx in 1..=count {
        let session_name = format!("{}{}", prefix, idx);
        let mut conn = PgConnection::connect(&doorman_addr)
            .await
            .expect("Failed to connect to pg_doorman");
        conn.send_startup(&user, &database)
            .await
            .expect("Failed to send startup to pg_doorman");
        conn.authenticate(&user, &password)
            .await
            .expect("Failed to authenticate to pg_doorman");
        world.named_sessions.insert(session_name, conn);
    }
}

#[when(
    regex = r#"^we create TLS session "([^"]+)" to pg_doorman as "([^"]+)" with password "([^"]*)" and database "([^"]+)"$"#
)]
pub async fn create_tls_named_session(
    world: &mut DoormanWorld,
    session_name: String,
    user: String,
    password: String,
    database: String,
) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

    let mut conn = PgConnection::connect(&doorman_addr)
        .await
        .expect("Failed to connect to pg_doorman");
    conn.upgrade_to_tls()
        .await
        .expect("Failed to upgrade to TLS");
    conn.send_startup(&user, &database)
        .await
        .expect("Failed to send startup to pg_doorman");
    conn.authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to pg_doorman");

    world.named_sessions.insert(session_name, conn);
}

#[when(
    regex = r#"^we create session "([^"]+)" to postgres as "([^"]+)" with password "([^"]*)" and database "([^"]+)"$"#
)]
pub async fn create_named_session_to_postgres(
    world: &mut DoormanWorld,
    session_name: String,
    user: String,
    password: String,
    database: String,
) {
    let pg_port = world.pg_port.expect("PostgreSQL not started");
    let pg_addr = format!("127.0.0.1:{}", pg_port);

    let mut conn = PgConnection::connect(&pg_addr)
        .await
        .expect("Failed to connect to PostgreSQL");
    conn.send_startup(&user, &database)
        .await
        .expect("Failed to send startup to PostgreSQL");
    conn.authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to PostgreSQL");

    world.named_sessions.insert(session_name, conn);
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)"$"#)]
pub async fn send_simple_query_to_session(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    let _messages = super::helpers::send_simple_query_and_read_until_ready(conn, &query).await;
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" and store backend_pid$"#)]
pub async fn send_simple_query_and_store_backend_pid(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    let backend_pid = super::helpers::read_first_datarow_int_until_ready(conn).await;

    if let Some(pid) = backend_pid {
        world.session_backend_pids.insert(session_name, pid);
    }
}

#[when(
    regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" and store backend_pid as "([^"]+)"$"#
)]
pub async fn send_simple_query_and_store_named_backend_pid(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
    pid_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    let backend_pid = super::helpers::read_first_datarow_int_until_ready(conn).await;

    if let Some(pid) = backend_pid {
        world
            .named_backend_pids
            .insert((session_name, pid_name), pid);
    }
}

#[when(regex = r#"^we sleep (\d+)ms$"#)]
#[when(regex = r#"^we sleep for (\d+) milliseconds$"#)]
pub async fn sleep_ms(_world: &mut DoormanWorld, ms: String) {
    let duration = ms.parse::<u64>().expect("Invalid sleep duration");
    tokio::time::sleep(tokio::time::Duration::from_millis(duration)).await;
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" without waiting$"#)]
pub async fn send_simple_query_to_session_without_waiting(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");
}

#[when(
    regex = r#"^we send SimpleQuery "([^"]+)" to (\d+) sessions with prefix "([^"]+)" without waiting$"#
)]
pub async fn send_simple_query_to_sessions_with_prefix_without_waiting(
    world: &mut DoormanWorld,
    query: String,
    count: usize,
    prefix: String,
) {
    for idx in 1..=count {
        let session_name = format!("{}{}", prefix, idx);
        let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);
        conn.send_simple_query(&query)
            .await
            .expect("Failed to send query");
    }
}

#[then(regex = r#"^we read SimpleQuery response from session "([^"]+)" within (\d+)ms$"#)]
pub async fn read_simple_query_response_within_timeout(
    world: &mut DoormanWorld,
    session_name: String,
    timeout_ms: u64,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    let duration = std::time::Duration::from_millis(timeout_ms);
    let messages = tokio::time::timeout(duration, conn.read_all_messages_until_ready())
        .await
        .unwrap_or_else(|_| panic!("Response not received within {}ms", timeout_ms))
        .expect("Failed to read messages");

    world.session_messages.insert(session_name, messages);
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" and store response$"#)]
#[then(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" and store response$"#)]
pub async fn send_simple_query_and_store_response(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    let messages = conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages");

    world
        .session_messages
        .insert(session_name.clone(), messages);
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" expecting error$"#)]
pub async fn send_simple_query_to_session_expecting_error(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    let messages = conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages");

    world
        .session_messages
        .insert(session_name.clone(), messages);
}

#[when(
    regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" expecting error after ready$"#
)]
pub async fn send_simple_query_to_session_expecting_error_after_ready(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    let messages = conn
        .read_all_messages_until_ready_and_more()
        .await
        .expect("Failed to read messages");

    world
        .session_messages
        .insert(session_name.clone(), messages);
}

#[when(
    regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" expecting connection close$"#
)]
pub async fn send_simple_query_to_session_expecting_connection_close(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    if conn.send_simple_query(&query).await.is_err() {
        return;
    }

    match conn.read_all_messages_until_ready().await {
        Ok(messages) => {
            let has_error = messages.iter().any(|(msg_type, _)| *msg_type == 'E');
            if has_error {
                world
                    .session_messages
                    .insert(session_name.clone(), messages);
                return;
            }
            panic!(
                "Expected connection close or error for session '{}', but got successful response",
                session_name
            );
        }
        Err(_) => {}
    }
}

#[when(regex = r#"^we send Parse "([^"]*)" with query "([^"]+)" to session "([^"]+)"$"#)]
#[then(regex = r#"^we send Parse "([^"]*)" with query "([^"]+)" to session "([^"]+)"$"#)]
pub async fn send_parse_to_session(
    world: &mut DoormanWorld,
    name: String,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_parse(&name, &query)
        .await
        .expect("Failed to send Parse");
}

#[when(
    regex = r#"^we send Bind "([^"]*)" to "([^"]*)" with params "([^"]*)" to session "([^"]+)"$"#
)]
#[then(
    regex = r#"^we send Bind "([^"]*)" to "([^"]*)" with params "([^"]*)" to session "([^"]+)"$"#
)]
pub async fn send_bind_to_session(
    world: &mut DoormanWorld,
    portal: String,
    statement: String,
    params_str: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    let params = super::helpers::parse_bind_params(&params_str);

    conn.send_bind(&portal, &statement, params)
        .await
        .expect("Failed to send Bind");
}

#[when(regex = r#"^we send FunctionCall (\d+) with int args "([^"]*)" to session "([^"]+)"$"#)]
#[then(regex = r#"^we send FunctionCall (\d+) with int args "([^"]*)" to session "([^"]+)"$"#)]
pub async fn send_function_call_to_session(
    world: &mut DoormanWorld,
    function_id: i32,
    args_str: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    let args = args_str
        .split(',')
        .map(str::trim)
        .filter(|arg| !arg.is_empty())
        .map(|arg| {
            arg.parse::<i32>()
                .unwrap_or_else(|_| panic!("Invalid FunctionCall int arg '{}'", arg))
        })
        .collect::<Vec<_>>();

    conn.send_function_call(function_id, &args)
        .await
        .expect("Failed to send FunctionCall");
}

#[then(regex = r#"^we read PostgreSQL response from session "([^"]+)" within (\d+)ms$"#)]
pub async fn read_postgresql_response_within_timeout(
    world: &mut DoormanWorld,
    session_name: String,
    timeout_ms: u64,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    let duration = std::time::Duration::from_millis(timeout_ms);
    let messages = tokio::time::timeout(duration, conn.read_all_messages_until_ready())
        .await
        .unwrap_or_else(|_| panic!("Response not received within {}ms", timeout_ms))
        .expect("Failed to read messages");

    world.session_messages.insert(session_name, messages);
}

#[when(regex = r#"^we send Execute "([^"]*)" to session "([^"]+)"$"#)]
#[then(regex = r#"^we send Execute "([^"]*)" to session "([^"]+)"$"#)]
pub async fn send_execute_to_session(
    world: &mut DoormanWorld,
    portal: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_execute(&portal, 0)
        .await
        .expect("Failed to send Execute");
}

#[when(regex = r#"^we send Flush to session "([^"]+)"$"#)]
#[then(regex = r#"^we send Flush to session "([^"]+)"$"#)]
pub async fn send_flush_to_session(world: &mut DoormanWorld, session_name: String) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_flush().await.expect("Failed to send Flush");
}

#[when(regex = r#"^we send Sync to session "([^"]+)"$"#)]
#[then(regex = r#"^we send Sync to session "([^"]+)"$"#)]
pub async fn send_sync_to_session(world: &mut DoormanWorld, session_name: String) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_sync().await.expect("Failed to send Sync");

    let messages = conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages");

    world
        .session_messages
        .insert(session_name.clone(), messages);
}

#[when(regex = r#"^we close session "([^"]+)"$"#)]
pub async fn close_session(world: &mut DoormanWorld, session_name: String) {
    world
        .named_sessions
        .remove(&session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name));
}

#[when(regex = r#"^we close (\d+) sessions with prefix "([^"]+)"$"#)]
pub async fn close_sessions_with_prefix(world: &mut DoormanWorld, count: usize, prefix: String) {
    for idx in 1..=count {
        let session_name = format!("{}{}", prefix, idx);
        world
            .named_sessions
            .remove(&session_name)
            .unwrap_or_else(|| panic!("Session '{}' not found", session_name));
    }
}

#[when(regex = r#"^we abort TCP connection for session "([^"]+)"$"#)]
pub async fn abort_session_tcp_connection(world: &mut DoormanWorld, session_name: String) {
    let conn = world
        .named_sessions
        .remove(&session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name));

    conn.abort_connection().await;
}

#[when(regex = r#"^we abort TCP connection with RST for session "([^"]+)"$"#)]
pub async fn abort_session_tcp_connection_with_rst(world: &mut DoormanWorld, session_name: String) {
    let conn = world
        .named_sessions
        .remove(&session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name));

    conn.abort_connection_with_rst().await;
}

#[then(regex = r#"^session "([^"]+)" should receive DataRow with "([^"]+)"$"#)]
pub async fn session_should_receive_datarow(
    world: &mut DoormanWorld,
    session_name: String,
    expected_value: String,
) {
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No messages stored for session '{}'", session_name));

    let mut found_value: Option<String> = None;
    for (msg_type, data) in messages {
        match msg_type {
            'D' => {
                let fields = super::helpers::parse_datarow_fields(data);
                if let Some(first) = fields.into_iter().next() {
                    found_value = Some(first);
                    break;
                }
            }
            'E' => {
                panic!(
                    "Error received from session '{}': {:?}",
                    session_name,
                    String::from_utf8_lossy(data)
                );
            }
            _ => {}
        }
    }

    let actual_value = found_value.unwrap_or_else(|| {
        panic!(
            "No DataRow received from session '{}', expected '{}'",
            session_name, expected_value
        )
    });

    assert_eq!(
        actual_value, expected_value,
        "Session '{}': expected '{}', got '{}'",
        session_name, expected_value, actual_value
    );
}

#[then(regex = r#"^session "([^"]+)" should receive ParseComplete$"#)]
pub async fn session_should_receive_parse_complete(world: &mut DoormanWorld, session_name: String) {
    expect_message_tag(world, &session_name, '1', "ParseComplete", None);
}

#[then(regex = r#"^session "([^"]+)" should receive BindComplete$"#)]
pub async fn session_should_receive_bind_complete(world: &mut DoormanWorld, session_name: String) {
    expect_message_tag(world, &session_name, '2', "BindComplete", None);
}

#[then(regex = r#"^session "([^"]+)" should receive FunctionCallResponse$"#)]
pub async fn session_should_receive_function_call_response(
    world: &mut DoormanWorld,
    session_name: String,
) {
    expect_message_tag(world, &session_name, 'V', "FunctionCallResponse", None);
}

#[then(regex = r#"^session "([^"]+)" should receive FunctionCallResponse with (\d+) byte result$"#)]
pub async fn session_should_receive_function_call_response_with_result_size(
    world: &mut DoormanWorld,
    session_name: String,
    expected_size: usize,
) {
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No messages stored for session '{}'", session_name));

    for (msg_type, data) in messages {
        if *msg_type != 'V' {
            continue;
        }
        assert!(
            data.len() >= 4,
            "FunctionCallResponse from session '{}' is too short: {} bytes",
            session_name,
            data.len(),
        );

        let result_len = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        assert!(result_len >= 0, "FunctionCallResponse result is NULL");
        let result_len = result_len as usize;
        assert_eq!(
            result_len, expected_size,
            "FunctionCallResponse from session '{}' has {} byte result, expected {}",
            session_name, result_len, expected_size,
        );
        assert_eq!(
            data.len(),
            4 + expected_size,
            "FunctionCallResponse from session '{}' has unexpected frame body size",
            session_name,
        );
        return;
    }

    panic!(
        "No FunctionCallResponse received from session '{}'",
        session_name
    );
}

#[then(regex = r#"^session "([^"]+)" should receive CommandComplete "([^"]+)"$"#)]
pub async fn session_should_receive_command_complete(
    world: &mut DoormanWorld,
    session_name: String,
    expected_tag: String,
) {
    expect_message_tag(
        world,
        &session_name,
        'C',
        "CommandComplete",
        Some(expected_tag.as_bytes()),
    );
}

#[then(regex = r#"^session "([^"]+)" should receive ReadyForQuery "([^"]+)"$"#)]
pub async fn session_should_receive_ready_for_query(
    world: &mut DoormanWorld,
    session_name: String,
    expected_status: String,
) {
    let expected = expected_status.as_bytes();
    assert_eq!(
        expected.len(),
        1,
        "ReadyForQuery status must be a single byte ('I'/'T'/'E'), got {:?}",
        expected_status
    );
    expect_message_tag(world, &session_name, 'Z', "ReadyForQuery", Some(expected));
}

/// Scan stored session messages for a frame with the given backend tag.
/// When `expected_body_prefix` is `Some`, the matching frame's body must
/// also start with those bytes — the caller uses this for tag-bearing
/// messages such as CommandComplete ("BEGIN\0") and ReadyForQuery ("I").
fn expect_message_tag(
    world: &DoormanWorld,
    session_name: &str,
    tag: char,
    label: &str,
    expected_body_prefix: Option<&[u8]>,
) {
    let messages = world
        .session_messages
        .get(session_name)
        .unwrap_or_else(|| panic!("No messages stored for session '{}'", session_name));

    for (msg_type, data) in messages {
        if *msg_type != tag {
            continue;
        }
        let Some(expected) = expected_body_prefix else {
            return;
        };
        if data.starts_with(expected) {
            return;
        }
    }

    panic!(
        "No {} received from session '{}' (looking for tag '{}'{})",
        label,
        session_name,
        tag,
        expected_body_prefix
            .map(|p| format!(" with body starting {:?}", String::from_utf8_lossy(p)))
            .unwrap_or_default(),
    );
}

#[then(regex = r#"^session "([^"]+)" should receive error containing "([^"]+)"$"#)]
pub async fn session_should_receive_error_containing(
    world: &mut DoormanWorld,
    session_name: String,
    expected_text: String,
) {
    // Get messages from the stored session messages
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No messages stored for session '{}'", session_name));

    let mut found_error: Option<String> = None;
    for (msg_type, data) in messages {
        if *msg_type == 'E' {
            let error_str = String::from_utf8_lossy(data).to_string();
            found_error = Some(error_str);
            break;
        }
    }

    let error_msg = found_error.unwrap_or_else(|| {
        panic!(
            "No ErrorResponse received from session '{}', expected error containing '{}'",
            session_name, expected_text
        )
    });

    assert!(
        error_msg
            .to_lowercase()
            .contains(&expected_text.to_lowercase()),
        "Session '{}': expected error containing '{}', got '{}'",
        session_name,
        expected_text,
        error_msg
    );
}

#[then(
    regex = r#"^session "([^"]+)" should receive error containing "([^"]+)" with code "([^"]+)"$"#
)]
pub async fn session_should_receive_error_containing_with_code(
    world: &mut DoormanWorld,
    session_name: String,
    expected_text: String,
    expected_code: String,
) {
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No messages stored for session '{}'", session_name));

    let mut found_error: Option<(String, String)> = None;
    for (msg_type, data) in messages {
        if *msg_type == 'E' {
            // Format: S<severity>\0 V<severity>\0 C<code>\0 M<message>\0 ... \0
            let mut code = String::new();
            let mut message = String::new();
            let mut i = 0;
            while i < data.len() {
                let field_type = data[i] as char;
                if field_type == '\0' {
                    break;
                }
                i += 1;
                let start = i;
                while i < data.len() && data[i] != 0 {
                    i += 1;
                }
                let value = String::from_utf8_lossy(&data[start..i]).to_string();
                i += 1; // skip null terminator
                match field_type {
                    'C' => code = value,
                    'M' => message = value,
                    _ => {}
                }
            }
            found_error = Some((message, code));
            break;
        }
    }

    let (error_msg, error_code) = found_error.unwrap_or_else(|| {
        panic!(
            "No ErrorResponse received from session '{}', expected error containing '{}' with code '{}'",
            session_name, expected_text, expected_code
        )
    });

    assert!(
        error_msg
            .to_lowercase()
            .contains(&expected_text.to_lowercase()),
        "Session '{}': expected error containing '{}', got '{}'",
        session_name,
        expected_text,
        error_msg
    );

    assert_eq!(
        error_code, expected_code,
        "Session '{}': expected error code '{}', got '{}'",
        session_name, expected_code, error_code
    );
}

#[when(
    regex = r#"^we send CopyFromStdin "([^"]+)" with data "([^"]*)" to session "([^"]+)" expecting error$"#
)]
pub async fn send_copy_from_stdin_to_session_expecting_error(
    world: &mut DoormanWorld,
    query: String,
    data: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    let unescaped_data = data.replace("\\t", "\t").replace("\\n", "\n");

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send COPY query");

    let (msg_type, msg_data) = conn
        .read_message()
        .await
        .expect("Failed to read COPY response");

    let mut messages: Vec<(char, Vec<u8>)> = Vec::new();

    if msg_type == 'G' {
        if !unescaped_data.is_empty() {
            conn.send_copy_data(unescaped_data.as_bytes())
                .await
                .expect("Failed to send CopyData");
        }
        conn.send_copy_done()
            .await
            .expect("Failed to send CopyDone");
    } else {
        messages.push((msg_type, msg_data));
    }

    let remaining = conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages");
    messages.extend(remaining);

    world.session_messages.insert(session_name, messages);
}
