@rust @pipeline-cancel-disconnect-bug
Feature: Pipeline cancel disconnect bug (many small rows, TCP RST)
  When a client sends extended protocol commands (Parse/Bind/Execute/Sync) that produce
  many small DataRow messages (each < 1MB but total ~10MB) and then abruptly kills
  the TCP connection with RST while pg_doorman is buffering/writing responses,
  pg_doorman must clean up the server connection before reuse.

  Bug: in execute_server_roundtrip, when write_all_flush to client fails with BrokenPipe
  in non-async non-copy mode, pg_doorman calls wait_available() but does NOT mark the
  server connection as bad, and does NOT return an error. The connection returns to the
  pool with dirty state.

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

  @pipeline-cancel-disconnect-bug
  Scenario: Client reads partial response of many small rows then RST - next client gets clean connection
    # Client A: query that returns many small rows (~10MB total, each row < 1MB)
    # This avoids the handle_large_data_row streaming path and goes through normal buffering
    When we create session "client_a" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # 20000 rows × ~512 bytes each ≈ 10MB of small DataRow messages
    And we send Parse "many_rows" with query "SELECT generate_series(1, 20000) as num, repeat('X', 512) as data" to session "client_a"
    And we send Bind "" to "many_rows" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Sync to session "client_a" without waiting for response

    # Read some data so pg_doorman enters the write-to-client path in roundtrip loop
    And we read 8192 bytes from session "client_a"

    # RST the connection - pg_doorman should get BrokenPipe on next write_all_flush
    When we abort TCP connection with RST for session "client_a"

    # Let pg_doorman handle the disconnect
    And we sleep for 2000 milliseconds

    # Client B: must get a clean connection
    When we create session "client_b" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "check" with query "SELECT 'CLEAN_CONNECTION'::text as status" to session "client_b"
    And we send Sync to session "client_b"
    And we send Bind "" to "check" with params "" to session "client_b"
    And we send Execute "" to session "client_b"
    And we send Sync to session "client_b"
    Then session "client_b" should receive DataRow with "CLEAN_CONNECTION"

  @pipeline-cancel-disconnect-bug-2
  Scenario: Client reads partial multi-batch many-rows response then RST - next client works
    When we create session "client_a" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Two queries each producing many small rows
    And we send Parse "q1" with query "SELECT generate_series(1, 10000) as n, repeat('A', 256) as d" to session "client_a"
    And we send Parse "q2" with query "SELECT generate_series(1, 10000) as n, repeat('B', 256) as d" to session "client_a"
    And we send Bind "" to "q1" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Bind "" to "q2" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Sync to session "client_a" without waiting for response

    # Read partial data
    And we read 16384 bytes from session "client_a"

    # RST while pg_doorman still writing
    When we abort TCP connection with RST for session "client_a"
    And we sleep for 2000 milliseconds

    When we create session "client_b" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "verify" with query "SELECT 'NO_CORRUPTION'::text as result" to session "client_b"
    And we send Sync to session "client_b"
    And we send Bind "" to "verify" with params "" to session "client_b"
    And we send Execute "" to session "client_b"
    And we send Sync to session "client_b"
    Then session "client_b" should receive DataRow with "NO_CORRUPTION"

  @pipeline-cancel-disconnect-bug-3
  Scenario: Repeated partial-read RST abort with many rows - connection stays usable
    When we create session "c1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s" with query "SELECT generate_series(1, 20000), repeat('X', 512)" to session "c1"
    And we send Bind "" to "s" with params "" to session "c1"
    And we send Execute "" to session "c1"
    And we send Sync to session "c1" without waiting for response
    And we read 4096 bytes from session "c1"
    When we abort TCP connection with RST for session "c1"
    And we sleep for 2000 milliseconds

    When we create session "c2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s" with query "SELECT generate_series(1, 20000), repeat('Y', 512)" to session "c2"
    And we send Bind "" to "s" with params "" to session "c2"
    And we send Execute "" to session "c2"
    And we send Sync to session "c2" without waiting for response
    And we read 4096 bytes from session "c2"
    When we abort TCP connection with RST for session "c2"
    And we sleep for 2000 milliseconds

    When we create session "c3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s" with query "SELECT generate_series(1, 20000), repeat('Z', 512)" to session "c3"
    And we send Bind "" to "s" with params "" to session "c3"
    And we send Execute "" to session "c3"
    And we send Sync to session "c3" without waiting for response
    And we read 4096 bytes from session "c3"
    When we abort TCP connection with RST for session "c3"
    And we sleep for 2000 milliseconds

    # Final client must work correctly
    When we create session "final" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "ok" with query "SELECT 'SURVIVED'::text as status" to session "final"
    And we send Sync to session "final"
    And we send Bind "" to "ok" with params "" to session "final"
    And we send Execute "" to session "final"
    And we send Sync to session "final"
    Then session "final" should receive DataRow with "SURVIVED"
