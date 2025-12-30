use crate::pg_connection::PgConnection;
use crate::world::DoormanWorld;
use cucumber::{then, when};

// BDD step implementations

#[when(
    regex = r#"^we login to postgres and pg_doorman as "([^"]+)" with password "([^"]*)" and database "([^"]+)"$"#
)]
pub async fn login_to_both(
    world: &mut DoormanWorld,
    user: String,
    password: String,
    database: String,
) {
    let pg_port = world.pg_port.expect("PostgreSQL not started");
    let doorman_port = world.doorman_port.expect("pg_doorman not started");

    let pg_addr = format!("127.0.0.1:{}", pg_port);
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

    // Connect to PostgreSQL
    let mut pg_conn = PgConnection::connect(&pg_addr)
        .await
        .expect("Failed to connect to PostgreSQL");
    pg_conn
        .send_startup(&user, &database)
        .await
        .expect("Failed to send startup to PostgreSQL");
    pg_conn
        .authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to PostgreSQL");

    // Connect to pg_doorman
    let mut doorman_conn = PgConnection::connect(&doorman_addr)
        .await
        .expect("Failed to connect to pg_doorman");
    doorman_conn
        .send_startup(&user, &database)
        .await
        .expect("Failed to send startup to pg_doorman");
    doorman_conn
        .authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to pg_doorman");

    world.pg_conn = Some(pg_conn);
    world.doorman_conn = Some(doorman_conn);
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to both$"#)]
pub async fn send_simple_query_to_both(world: &mut DoormanWorld, query: String) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    pg_conn
        .send_simple_query(&query)
        .await
        .expect("Failed to send query to PostgreSQL");
    doorman_conn
        .send_simple_query(&query)
        .await
        .expect("Failed to send query to pg_doorman");

    // Read messages from both
    let pg_messages = pg_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages from PostgreSQL");
    let doorman_messages = doorman_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages from pg_doorman");

    world.pg_accumulated_messages.extend(pg_messages);
    world.doorman_accumulated_messages.extend(doorman_messages);
}

#[when(regex = r#"^we send Parse "([^"]*)" with query "([^"]+)" to both$"#)]
pub async fn send_parse_to_both(world: &mut DoormanWorld, name: String, query: String) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    pg_conn
        .send_parse(&name, &query)
        .await
        .expect("Failed to send Parse to PostgreSQL");
    doorman_conn
        .send_parse(&name, &query)
        .await
        .expect("Failed to send Parse to pg_doorman");
}

#[when(regex = r#"^we send Bind "([^"]*)" to "([^"]*)" with params "([^"]+)" to both$"#)]
pub async fn send_bind_to_both(
    world: &mut DoormanWorld,
    portal: String,
    statement: String,
    params_str: String,
) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    // Parse params - simple implementation for comma-separated values
    let params: Vec<Option<Vec<u8>>> = params_str
        .split(',')
        .map(|s| Some(s.trim().as_bytes().to_vec()))
        .collect();

    pg_conn
        .send_bind(&portal, &statement, params.clone())
        .await
        .expect("Failed to send Bind to PostgreSQL");
    doorman_conn
        .send_bind(&portal, &statement, params)
        .await
        .expect("Failed to send Bind to pg_doorman");
}

#[when(regex = r#"^we send Describe "([^"])" "([^"]*)" to both$"#)]
pub async fn send_describe_to_both(world: &mut DoormanWorld, target_type: String, name: String) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    let target_char = target_type.chars().next().expect("Empty target type");

    pg_conn
        .send_describe(target_char, &name)
        .await
        .expect("Failed to send Describe to PostgreSQL");
    doorman_conn
        .send_describe(target_char, &name)
        .await
        .expect("Failed to send Describe to pg_doorman");
}

#[when(regex = r#"^we send Execute "([^"]*)" to both$"#)]
pub async fn send_execute_to_both(world: &mut DoormanWorld, portal: String) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    pg_conn
        .send_execute(&portal, 0)
        .await
        .expect("Failed to send Execute to PostgreSQL");
    doorman_conn
        .send_execute(&portal, 0)
        .await
        .expect("Failed to send Execute to pg_doorman");
}

#[when(regex = r#"^we send Sync to both$"#)]
pub async fn send_sync_to_both(world: &mut DoormanWorld) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    pg_conn
        .send_sync()
        .await
        .expect("Failed to send Sync to PostgreSQL");
    doorman_conn
        .send_sync()
        .await
        .expect("Failed to send Sync to pg_doorman");

    // Read messages from both
    let pg_messages = pg_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages from PostgreSQL");
    let doorman_messages = doorman_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages from pg_doorman");

    world.pg_accumulated_messages.extend(pg_messages);
    world.doorman_accumulated_messages.extend(doorman_messages);
}

#[then(regex = r#"^we should receive identical messages from both$"#)]
pub async fn verify_identical_messages(world: &mut DoormanWorld) {
    let pg_messages = &world.pg_accumulated_messages;
    let doorman_messages = &world.doorman_accumulated_messages;

    assert_eq!(
        pg_messages.len(),
        doorman_messages.len(),
        "Number of messages differs: PostgreSQL={}, pg_doorman={}",
        pg_messages.len(),
        doorman_messages.len()
    );

    for (i, (pg_msg, doorman_msg)) in pg_messages.iter().zip(doorman_messages.iter()).enumerate() {
        let (pg_type, pg_data) = pg_msg;
        let (doorman_type, doorman_data) = doorman_msg;

        assert_eq!(
            pg_type, doorman_type,
            "Message {} type differs: PostgreSQL='{}', pg_doorman='{}'",
            i, pg_type, doorman_type
        );

        assert_eq!(
            pg_data.len(),
            doorman_data.len(),
            "Message {} length differs: PostgreSQL={}, pg_doorman={}",
            i,
            pg_data.len(),
            doorman_data.len()
        );

        assert_eq!(
            pg_data, doorman_data,
            "Message {} data differs: PostgreSQL={:?}, pg_doorman={:?}",
            i, pg_data, doorman_data
        );

        println!(
            "Message {} is identical: type='{}', length={}",
            i,
            pg_type,
            pg_data.len()
        );
    }

    // Clear accumulated messages for next scenario
    world.pg_accumulated_messages.clear();
    world.doorman_accumulated_messages.clear();
}
