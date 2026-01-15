@rust @extended-disconnect
Feature: Server buffer cleanup on client disconnect during extended protocol
  Test that pg_doorman correctly cleans up server buffer when client
  disconnects unexpectedly in the middle of extended protocol query.
  This ensures that the next client session doesn't receive "stale" data
  or protocol errors from the previous session's incomplete query.

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
      prepared_statements_cache_size = 10000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  @extended-disconnect-basic
  Scenario: Buffer is cleaned after client disconnects during extended protocol query
    # Session one: start an extended protocol query but don't read response
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Send Parse/Bind/Execute/Sync for a query that returns data
    And we send Parse "" with query "SELECT generate_series(1, 1000) as num" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one" without waiting
    # Read only partial data (8KB)
    And we read 8192 bytes from session "one"
    # Abruptly disconnect (TCP abort)
    And we abort TCP connection for session "one"
    # Wait for pg_doorman to detect disconnect and cleanup
    And we sleep 500ms
    # Session two: connect and run a simple query using extended protocol
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT 42 as answer" to session "two"
    And we send Bind "" to "" with params "" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    # The result should be exactly what we expect, no leftover data from session one
    Then session "two" should receive DataRow with "42"

  @extended-disconnect-large-result
  Scenario: Buffer cleanup works with large result set in extended protocol
    # Session one: start an extended protocol query with large result
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT repeat('X', 1048576) as large_data FROM generate_series(1, 8)" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one" without waiting
    # Read only partial data
    And we read 8192 bytes from session "one"
    # Abruptly disconnect
    And we abort TCP connection for session "one"
    And we sleep 500ms
    # Session two should work correctly
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT 'CLEAN_MARKER' as marker" to session "two"
    And we send Bind "" to "" with params "" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "CLEAN_MARKER"

  @extended-disconnect-multiple
  Scenario: Multiple disconnects during extended protocol don't leak data
    # First disconnect
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT generate_series(1, 10000) as num" to session "first"
    And we send Bind "" to "" with params "" to session "first"
    And we send Execute "" to session "first"
    And we send Sync to session "first" without waiting
    And we read 4096 bytes from session "first"
    And we abort TCP connection for session "first"
    And we sleep 300ms
    # Second disconnect
    When we create session "second" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT generate_series(1, 10000) as num" to session "second"
    And we send Bind "" to "" with params "" to session "second"
    And we send Execute "" to session "second"
    And we send Sync to session "second" without waiting
    And we read 4096 bytes from session "second"
    And we abort TCP connection for session "second"
    And we sleep 300ms
    # Third disconnect
    When we create session "third" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT generate_series(1, 10000) as num" to session "third"
    And we send Bind "" to "" with params "" to session "third"
    And we send Execute "" to session "third"
    And we send Sync to session "third" without waiting
    And we read 4096 bytes from session "third"
    And we abort TCP connection for session "third"
    And we sleep 300ms
    # Final session should get clean data
    When we create session "final" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT 'FINAL_CLEAN' as marker" to session "final"
    And we send Bind "" to "" with params "" to session "final"
    And we send Execute "" to session "final"
    And we send Sync to session "final"
    Then session "final" should receive DataRow with "FINAL_CLEAN"

  @extended-disconnect-with-params
  Scenario: Buffer cleanup works with parameterized extended protocol query
    # Session one: parameterized query with disconnect
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT $1::int + generate_series(1, 10000) as num" to session "one"
    And we send Bind "" to "" with params "100" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one" without waiting
    And we read 4096 bytes from session "one"
    And we abort TCP connection for session "one"
    And we sleep 500ms
    # Session two with different params should work
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT $1::int as answer" to session "two"
    And we send Bind "" to "" with params "999" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "999"
