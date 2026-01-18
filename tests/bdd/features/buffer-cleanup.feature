@rust @buffer-cleanup
Feature: Server buffer cleanup on client disconnect
  Test that pg_doorman correctly cleans up server buffer when client
  disconnects unexpectedly in the middle of receiving large result set.
  This ensures that the next client session doesn't receive "stale" data
  from the previous session's query.

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

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  @buffer-cleanup-large-result
  Scenario: Buffer is cleaned after client disconnects during large result transfer
    # Session one: start a query that returns 8MB of text data
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Generate 8MB of data using repeat() - each row is about 1MB
    And we send SimpleQuery "SELECT repeat('X', 1048576) as large_data FROM generate_series(1, 8)" to session "one" without waiting for response
    # Read only 8KB of data (partial read)
    And we read 8192 bytes from session "one"
    # Abruptly disconnect (TCP abort)
    And we abort TCP connection for session "one"
    # Wait for pg_doorman to detect disconnect and cleanup
    And we sleep 500ms
    # Session two: connect and run a simple query
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Run a query that returns a known, small result
    And we send SimpleQuery "SELECT 'CLEAN_SESSION_MARKER' as marker" to session "two" and verify no stale data
    # The result should be exactly what we expect, no leftover data from session one
    Then session "two" should have received clean response with marker "CLEAN_SESSION_MARKER"

  @buffer-cleanup-multiple-disconnects
  Scenario: Multiple disconnects during large transfers don't leak data
    # First disconnect
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('A', 1048576) as data FROM generate_series(1, 4)" to session "first" without waiting for response
    And we read 4096 bytes from session "first"
    And we abort TCP connection for session "first"
    And we sleep 300ms
    # Second disconnect
    When we create session "second" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('B', 1048576) as data FROM generate_series(1, 4)" to session "second" without waiting for response
    And we read 4096 bytes from session "second"
    And we abort TCP connection for session "second"
    And we sleep 300ms
    # Third disconnect
    When we create session "third" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('C', 1048576) as data FROM generate_series(1, 4)" to session "third" without waiting for response
    And we read 4096 bytes from session "third"
    And we abort TCP connection for session "third"
    And we sleep 300ms
    # Final session should get clean data
    When we create session "final" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'FINAL_CLEAN_MARKER' as marker" to session "final" and verify no stale data
    Then session "final" should have received clean response with marker "FINAL_CLEAN_MARKER"

  @buffer-cleanup-transaction-rollback
  Scenario: Buffer cleanup works correctly when disconnect happens in transaction
    # Start a transaction and begin large query
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "one"
    And we send SimpleQuery "SELECT repeat('T', 1048576) as data FROM generate_series(1, 8)" to session "one" without waiting for response
    And we read 8192 bytes from session "one"
    And we abort TCP connection for session "one"
    And we sleep 500ms
    # New session should work correctly
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'TRANSACTION_CLEAN_MARKER' as marker" to session "two" and verify no stale data
    Then session "two" should have received clean response with marker "TRANSACTION_CLEAN_MARKER"
