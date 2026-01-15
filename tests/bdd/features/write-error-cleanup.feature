@rust @write-error-cleanup
Feature: Server cleanup on client write error
  Test that pg_doorman correctly handles the case when client disconnects
  while server is sending data. The server connection should be properly
  cleaned up and not returned to the pool in a dirty state.

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

      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  @write-error-simple-query
  Scenario: Server is cleaned up when client disconnects during SimpleQuery response
    # Session one: start a query that returns large result
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Query returns many rows to ensure server has data_available = true
    And we send SimpleQuery "SELECT generate_series(1, 100000) as num" to session "one" without waiting for response
    # Read partial data to ensure server started sending
    And we read 4096 bytes from session "one"
    # Abruptly disconnect - this should trigger write error in pg_doorman
    And we abort TCP connection for session "one"
    # Wait for cleanup
    And we sleep 500ms
    # Session two should get a clean connection (possibly new one if old was marked bad)
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 42 as answer" to session "two" and verify no stale data
    Then session "two" should have received clean response with marker "42"

  @write-error-multiple-queries
  Scenario: Multiple write errors don't corrupt pool state
    # First write error
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT generate_series(1, 50000) as num" to session "first" without waiting for response
    And we read 2048 bytes from session "first"
    And we abort TCP connection for session "first"
    And we sleep 300ms
    # Second write error
    When we create session "second" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT generate_series(1, 50000) as num" to session "second" without waiting for response
    And we read 2048 bytes from session "second"
    And we abort TCP connection for session "second"
    And we sleep 300ms
    # Third write error
    When we create session "third" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT generate_series(1, 50000) as num" to session "third" without waiting for response
    And we read 2048 bytes from session "third"
    And we abort TCP connection for session "third"
    And we sleep 300ms
    # Final session should work correctly
    When we create session "final" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'CLEAN' as status" to session "final" and verify no stale data
    Then session "final" should have received clean response with marker "CLEAN"

  @write-error-in-transaction
  Scenario: Write error during transaction properly cleans up server
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Start transaction
    And we send SimpleQuery "BEGIN" to session "one"
    # Query with large result inside transaction
    And we send SimpleQuery "SELECT generate_series(1, 100000) as num" to session "one" without waiting for response
    And we read 4096 bytes from session "one"
    # Disconnect during transaction
    And we abort TCP connection for session "one"
    And we sleep 500ms
    # New session should get clean connection with no active transaction
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # This query would fail if we inherited an aborted transaction
    And we send SimpleQuery "SELECT 'NO_TRANSACTION' as status" to session "two" and verify no stale data
    Then session "two" should have received clean response with marker "NO_TRANSACTION"

  @write-error-extended-protocol-sync
  Scenario: Write error during extended protocol with Sync properly cleans up
    # Extended protocol with Sync (not async mode)
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT generate_series(1, 100000) as num" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one" without waiting
    # Read partial response
    And we read 4096 bytes from session "one"
    # Disconnect
    And we abort TCP connection for session "one"
    And we sleep 500ms
    # New session should work
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT 123 as result" to session "two"
    And we send Bind "" to "" with params "" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "123"

  @write-error-rapid-reconnect
  Scenario: Rapid reconnect after write error gets clean connection
    # Disconnect during data transfer
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT generate_series(1, 100000) as num" to session "one" without waiting for response
    And we read 1024 bytes from session "one"
    And we abort TCP connection for session "one"
    # Immediately try to reconnect (minimal sleep)
    And we sleep 100ms
    # New session should still get clean data
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'RAPID_RECONNECT_OK' as status" to session "two" and verify no stale data
    Then session "two" should have received clean response with marker "RAPID_RECONNECT_OK"
