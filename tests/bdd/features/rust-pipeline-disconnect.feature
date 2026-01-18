@rust @rust-pipeline-disconnect
Feature: Rust raw protocol pipeline disconnect test
  Test that pg_doorman correctly handles client disconnect during pipeline/batch operations.
  Client A starts a batch query with large result and crashes (disconnects without reading all data).
  Client B should get a clean connection (same server connection from pool with pool_size=1).

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  Scenario: Client A crashes during pipeline batch - client B gets clean connection - pool_size=1
    # Client A sends multiple Parse/Bind/Execute commands (pipeline/batch mode)
    # then disconnects WITHOUT reading all results and WITHOUT sending final Sync
    When we create session "client_a" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Send multiple Parse commands (like batch.PrepareAsync)
    And we send Parse "batch_stmt_0" with query "SELECT 0 as batch_num, generate_series(1, 100) as num" to session "client_a"
    And we send Parse "batch_stmt_1" with query "SELECT 1 as batch_num, generate_series(1, 100) as num" to session "client_a"
    And we send Parse "batch_stmt_2" with query "SELECT 2 as batch_num, generate_series(1, 100) as num" to session "client_a"
    And we send Parse "batch_stmt_3" with query "SELECT 3 as batch_num, generate_series(1, 100) as num" to session "client_a"
    And we send Parse "batch_stmt_4" with query "SELECT 4 as batch_num, generate_series(1, 100) as num" to session "client_a"

    # Send Bind/Execute for all commands (like batch.ExecuteReaderAsync)
    And we send Bind "" to "batch_stmt_0" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Bind "" to "batch_stmt_1" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Bind "" to "batch_stmt_2" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Bind "" to "batch_stmt_3" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Bind "" to "batch_stmt_4" with params "" to session "client_a"
    And we send Execute "" to session "client_a"

    # Client A "crashes" - disconnects without sending Sync and without reading results
    # This leaves server with pending results that need to be drained
    When we close session "client_a"

    # Small delay to let pg_doorman detect the disconnect and clean up
    And we sleep for 200 milliseconds

    # Client B connects - should get clean connection, not garbage from client A
    When we create session "client_b" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Client B executes its own query and should get correct results
    And we send Parse "my_query" with query "SELECT 'CLIENT_B_SUCCESS'::text as result" to session "client_b"
    And we send Sync to session "client_b"
    And we send Bind "" to "my_query" with params "" to session "client_b"
    And we send Execute "" to session "client_b"
    And we send Sync to session "client_b"
    Then session "client_b" should receive DataRow with "CLIENT_B_SUCCESS"

  Scenario: Client A sends Sync but disconnects before reading all results - client B gets clean connection
    # Client A sends complete pipeline with Sync, but disconnects before reading all data
    When we create session "client_a" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Send Parse/Bind/Execute for multiple large result sets
    And we send Parse "large_query" with query "SELECT generate_series(1, 1000) as num, repeat('X', 100) as data" to session "client_a"
    And we send Bind "" to "large_query" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Sync to session "client_a"

    # Client A disconnects immediately without reading the large result
    When we close session "client_a"

    And we sleep for 200 milliseconds

    # Client B should get clean connection
    When we create session "client_b" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "check_query" with query "SELECT 'CLEAN_STATE'::text as status" to session "client_b"
    And we send Sync to session "client_b"
    And we send Bind "" to "check_query" with params "" to session "client_b"
    And we send Execute "" to session "client_b"
    And we send Sync to session "client_b"
    Then session "client_b" should receive DataRow with "CLEAN_STATE"

  Scenario: Multiple pipeline crashes in sequence - connection stays clean
    # Multiple clients crash during pipeline operations
    # Final client should still get clean connection

    # Client 1 crashes
    When we create session "crash1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "SELECT generate_series(1, 500) as n" to session "crash1"
    And we send Bind "" to "stmt" with params "" to session "crash1"
    And we send Execute "" to session "crash1"
    When we close session "crash1"

    # Client 2 crashes
    When we create session "crash2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "SELECT generate_series(1, 500) as n, repeat('Y', 50) as data" to session "crash2"
    And we send Bind "" to "stmt" with params "" to session "crash2"
    And we send Execute "" to session "crash2"
    And we send Sync to session "crash2"
    When we close session "crash2"

    # Client 3 crashes with multiple statements
    When we create session "crash3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "SELECT 1" to session "crash3"
    And we send Parse "s2" with query "SELECT 2" to session "crash3"
    And we send Parse "s3" with query "SELECT 3" to session "crash3"
    And we send Bind "" to "s1" with params "" to session "crash3"
    And we send Execute "" to session "crash3"
    And we send Bind "" to "s2" with params "" to session "crash3"
    And we send Execute "" to session "crash3"
    When we close session "crash3"

    # Final client should work correctly
    When we create session "final" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "final_stmt" with query "SELECT 'ALL_CLEAN'::text as result" to session "final"
    And we send Sync to session "final"
    And we send Bind "" to "final_stmt" with params "" to session "final"
    And we send Execute "" to session "final"
    And we send Sync to session "final"
    Then session "final" should receive DataRow with "ALL_CLEAN"

  Scenario: Interleaved normal and crashing clients - all normal clients work correctly
    # Good client 1 works normally
    When we create session "good1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "SELECT 'GOOD1'::text as val" to session "good1"
    And we send Sync to session "good1"
    And we send Bind "" to "stmt" with params "" to session "good1"
    And we send Execute "" to session "good1"
    And we send Sync to session "good1"
    Then session "good1" should receive DataRow with "GOOD1"

    # Crashing client with pipeline
    When we create session "crash" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "p1" with query "SELECT generate_series(1, 200)" to session "crash"
    And we send Parse "p2" with query "SELECT generate_series(1, 200)" to session "crash"
    And we send Bind "" to "p1" with params "" to session "crash"
    And we send Execute "" to session "crash"
    And we send Bind "" to "p2" with params "" to session "crash"
    And we send Execute "" to session "crash"
    When we close session "crash"

    # Good client 2 should work correctly
    When we create session "good2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "SELECT 'GOOD2'::text as val" to session "good2"
    And we send Sync to session "good2"
    And we send Bind "" to "stmt" with params "" to session "good2"
    And we send Execute "" to session "good2"
    And we send Sync to session "good2"
    Then session "good2" should receive DataRow with "GOOD2"

    # Good client 1 should still work
    When we send Bind "" to "stmt" with params "" to session "good1"
    And we send Execute "" to session "good1"
    And we send Sync to session "good1"
    Then session "good1" should receive DataRow with "GOOD1"

  Scenario: Client crashes after partial Sync - next client gets clean state
    # Client sends some commands, Sync, more commands, then crashes
    When we create session "partial" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # First batch with Sync
    And we send Parse "first" with query "SELECT 'FIRST'::text" to session "partial"
    And we send Bind "" to "first" with params "" to session "partial"
    And we send Execute "" to session "partial"
    And we send Sync to session "partial"

    # Second batch without Sync - client crashes here
    And we send Parse "second" with query "SELECT generate_series(1, 500)" to session "partial"
    And we send Bind "" to "second" with params "" to session "partial"
    And we send Execute "" to session "partial"
    # No Sync - crash!
    When we close session "partial"

    # Next client should get clean state
    When we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "check" with query "SELECT 'RECOVERED'::text as status" to session "next"
    And we send Sync to session "next"
    And we send Bind "" to "check" with params "" to session "next"
    And we send Execute "" to session "next"
    And we send Sync to session "next"
    Then session "next" should receive DataRow with "RECOVERED"
