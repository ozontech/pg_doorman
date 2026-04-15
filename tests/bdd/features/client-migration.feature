@client-migration
Feature: Client migration during binary upgrade
  On SIGUSR2, idle plain TCP clients should migrate from the old process
  to the new one. The session stays connected — the client never
  disconnects or re-authenticates.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  Scenario: Idle client continues working after binary upgrade
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    When we sleep 1000ms
    # Open a session and verify it works
    And we create session "migrated" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "migrated"
    # Store the old pg_doorman PID
    And we store foreground pg_doorman PID as "old_doorman"
    # Trigger binary upgrade — session "migrated" stays open
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # The same session should still work without reconnecting
    Then we send SimpleQuery "SELECT 42" to session "migrated" and store response
    And session "migrated" should receive DataRow with "42"
    # The old pg_doorman process should be gone — the client was migrated
    And stored foreground PID "old_doorman" should not exist
    When we close session "migrated"

  Scenario: Migrated session continues working after upgrade with multiple queries
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we sleep 1000ms
    And we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Multiple queries after migration to verify session is fully functional
    Then we send SimpleQuery "SELECT 'post_migrate_1'" to session "s1" and store response
    And session "s1" should receive DataRow with "post_migrate_1"
    And we send SimpleQuery "SELECT 'post_migrate_2'" to session "s1" and store response
    And session "s1" should receive DataRow with "post_migrate_2"
    And stored foreground PID "old_doorman" should not exist
    When we close session "s1"

  Scenario: Prepared statement survives migration
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      prepared_statements_cache_size = 100
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we sleep 1000ms
    And we create session "ps" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Create a named prepared statement
    And we send Parse "my_stmt" with query "SELECT 1 AS val" to session "ps"
    And we send Bind "" to "my_stmt" with params "" to session "ps"
    And we send Execute "" to session "ps"
    And we send Sync to session "ps"
    Then session "ps" should receive DataRow with "1"
    # Trigger binary upgrade
    When we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Bind to the same prepared statement after migration — cache was transferred
    And we send Bind "" to "my_stmt" with params "" to session "ps"
    And we send Execute "" to session "ps"
    And we send Sync to session "ps"
    Then session "ps" should receive DataRow with "1"
    And stored foreground PID "old_doorman" should not exist
    When we close session "ps"

  Scenario: Multiple prepared statements with parameters survive migration
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      prepared_statements_cache_size = 100
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we sleep 1000ms
    And we create session "mps" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Statement 1: no params
    And we send Parse "s1" with query "SELECT 100 AS val" to session "mps"
    And we send Bind "" to "s1" with params "" to session "mps"
    And we send Execute "" to session "mps"
    And we send Sync to session "mps"
    Then session "mps" should receive DataRow with "100"
    # Statement 2: int params
    When we send Parse "s2" with query "SELECT $1::int + $2::int AS sum" to session "mps"
    And we send Bind "" to "s2" with params "3,7" to session "mps"
    And we send Execute "" to session "mps"
    And we send Sync to session "mps"
    Then session "mps" should receive DataRow with "10"
    # Statement 3: text param
    When we send Parse "s3" with query "SELECT length($1::text) AS len" to session "mps"
    And we send Bind "" to "s3" with params "hello" to session "mps"
    And we send Execute "" to session "mps"
    And we send Sync to session "mps"
    Then session "mps" should receive DataRow with "5"
    # Migrate
    When we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Re-execute all three with different params after migration
    And we send Bind "" to "s1" with params "" to session "mps"
    And we send Execute "" to session "mps"
    And we send Sync to session "mps"
    Then session "mps" should receive DataRow with "100"
    When we send Bind "" to "s2" with params "10,20" to session "mps"
    And we send Execute "" to session "mps"
    And we send Sync to session "mps"
    Then session "mps" should receive DataRow with "30"
    When we send Bind "" to "s3" with params "migration" to session "mps"
    And we send Execute "" to session "mps"
    And we send Sync to session "mps"
    Then session "mps" should receive DataRow with "9"
    And stored foreground PID "old_doorman" should not exist
    When we close session "mps"

  Scenario: Anonymous prepared statement survives migration
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      prepared_statements_cache_size = 100
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we sleep 1000ms
    And we create session "anon" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Anonymous prepared statement (empty name)
    And we send Parse "" with query "SELECT $1::int * 2 AS doubled" to session "anon"
    And we send Bind "" to "" with params "21" to session "anon"
    And we send Execute "" to session "anon"
    And we send Sync to session "anon"
    Then session "anon" should receive DataRow with "42"
    # Migrate
    When we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Re-parse same anonymous statement after migration and execute
    And we send Parse "" with query "SELECT $1::int * 2 AS doubled" to session "anon"
    And we send Bind "" to "" with params "50" to session "anon"
    And we send Execute "" to session "anon"
    And we send Sync to session "anon"
    Then session "anon" should receive DataRow with "100"
    And stored foreground PID "old_doorman" should not exist
    When we close session "anon"

  Scenario: Client mid-transaction finishes then migrates
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 10000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    When we sleep 1000ms
    And we create session "tx" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Start a transaction — client holds a server connection
    And we send SimpleQuery "BEGIN" to session "tx"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "tx" and store backend_pid as "in_tx"
    And we store foreground pg_doorman PID as "old_doorman"
    # Trigger upgrade while transaction is active
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Transaction still works on old process (not migrated yet)
    And we send SimpleQuery "SELECT 1" to session "tx"
    # Commit releases the server — client becomes idle and migrates
    And we send SimpleQuery "COMMIT" to session "tx"
    # After commit, the next query goes through the new process
    And we send SimpleQuery "SELECT 42" to session "tx" and store response
    Then session "tx" should receive DataRow with "42"
    # Old process should exit once all clients migrated
    When we sleep 2000ms
    Then stored foreground PID "old_doorman" should not exist
    When we close session "tx"

  Scenario: Multiple clients migrate concurrently
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    When we sleep 1000ms
    And we create session "c1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "c2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "c3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "c1"
    And we send SimpleQuery "SELECT 1" to session "c2"
    And we send SimpleQuery "SELECT 1" to session "c3"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # All three sessions should work after migration
    Then we send SimpleQuery "SELECT 'c1_alive'" to session "c1" and store response
    And session "c1" should receive DataRow with "c1_alive"
    And we send SimpleQuery "SELECT 'c2_alive'" to session "c2" and store response
    And session "c2" should receive DataRow with "c2_alive"
    And we send SimpleQuery "SELECT 'c3_alive'" to session "c3" and store response
    And session "c3" should receive DataRow with "c3_alive"
    And stored foreground PID "old_doorman" should not exist
    When we close session "c1"
    And we close session "c2"
    And we close session "c3"
