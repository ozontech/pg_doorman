@rust @pipeline-async-disconnect
Feature: Async pipeline disconnect (Flush mode)
  When a client uses Flush (async/pipeline mode) instead of Sync, and disconnects
  before reading all responses, checkin_cleanup is skipped for async connections.
  The server connection returns to pool with async_mode=true, pending expected_responses,
  and potentially buffered data.

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

  @pipeline-async-disconnect-1
  Scenario: Client sends Flush pipeline then RST - next client must work
    # Client A: pipeline with Flush (enters async mode)
    When we create session "client_a" to pg_doorman as "example_user_1" with password "" and database "example_db"

    And we send Parse "q1" with query "SELECT generate_series(1, 5000) as n, repeat('X', 256) as d" to session "client_a"
    And we send Bind "" to "q1" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Flush to session "client_a"

    # Read a few bytes so pg_doorman enters roundtrip
    And we read 4096 bytes from session "client_a"

    # RST while async responses still pending
    When we abort TCP connection with RST for session "client_a"
    And we sleep for 2000 milliseconds

    # Client B: must get clean connection
    When we create session "client_b" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "check" with query "SELECT 'ASYNC_CLEAN'::text as status" to session "client_b"
    And we send Sync to session "client_b"
    And we send Bind "" to "check" with params "" to session "client_b"
    And we send Execute "" to session "client_b"
    And we send Sync to session "client_b"
    Then session "client_b" should receive DataRow with "ASYNC_CLEAN"

  @pipeline-async-disconnect-2
  Scenario: Client sends multiple Flush batches then RST - next client must work
    When we create session "client_a" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # First batch with Flush
    And we send Parse "q1" with query "SELECT 1 as batch1" to session "client_a"
    And we send Bind "" to "q1" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Flush to session "client_a"

    # Second batch with Flush (still async)
    And we send Parse "q2" with query "SELECT generate_series(1, 3000), repeat('Y', 512)" to session "client_a"
    And we send Bind "" to "q2" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Flush to session "client_a"

    And we read 4096 bytes from session "client_a"
    When we abort TCP connection with RST for session "client_a"
    And we sleep for 2000 milliseconds

    When we create session "client_b" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "check" with query "SELECT 'MULTI_FLUSH_CLEAN'::text as status" to session "client_b"
    And we send Sync to session "client_b"
    And we send Bind "" to "check" with params "" to session "client_b"
    And we send Execute "" to session "client_b"
    And we send Sync to session "client_b"
    Then session "client_b" should receive DataRow with "MULTI_FLUSH_CLEAN"

  @pipeline-async-disconnect-3
  Scenario: Client sends Flush with large result then RST - server connection reused
    When we create session "client_a" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Large result via Flush (async mode)
    And we send Parse "big" with query "SELECT generate_series(1, 10000) as n, repeat('Z', 512) as d" to session "client_a"
    And we send Bind "" to "big" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Flush to session "client_a"

    And we read 8192 bytes from session "client_a"
    When we abort TCP connection with RST for session "client_a"
    And we sleep for 2000 milliseconds

    # Repeat abort cycle
    When we create session "c2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "big" with query "SELECT generate_series(1, 10000) as n, repeat('W', 512) as d" to session "c2"
    And we send Bind "" to "big" with params "" to session "c2"
    And we send Execute "" to session "c2"
    And we send Flush to session "c2"
    And we read 8192 bytes from session "c2"
    When we abort TCP connection with RST for session "c2"
    And we sleep for 2000 milliseconds

    When we create session "final" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "ok" with query "SELECT 'FLUSH_SURVIVED'::text as result" to session "final"
    And we send Sync to session "final"
    And we send Bind "" to "ok" with params "" to session "final"
    And we send Execute "" to session "final"
    And we send Sync to session "final"
    Then session "final" should receive DataRow with "FLUSH_SURVIVED"
