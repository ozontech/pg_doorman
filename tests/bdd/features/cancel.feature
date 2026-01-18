@rust @cancel
Feature: Cancel request (pqCancel) functionality
  Test that pg_doorman correctly handles PostgreSQL cancel requests (pqCancel)
  which allows clients to cancel long-running queries

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

  @cancel-pg-sleep
  Scenario: Cancel a long-running pg_sleep query
    # Connect to pg_doorman and store backend_pid and secret_key
    When we create session "main" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    # Start a long-running query (pg_sleep for 10 seconds)
    And we send SimpleQuery "SELECT pg_sleep(10)" to session "main" without waiting for response
    # Wait a bit to ensure the query has started
    And we sleep 500ms
    # Send cancel request from a separate connection
    And we send cancel request for session "main"
    # Verify that the main session received a cancellation error
    Then session "main" should receive cancel error containing "canceling"

  @cancel-race
  Scenario: Cancel request from disconnected session should not kill another session
    # Create main session and store backend_pid/secret_key
    When we create session "main" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    # Execute a simple query to ensure connection is working
    And we send SimpleQuery "SELECT 1" to session "main"
    # Abruptly disconnect main session (simulates network failure)
    And we abort TCP connection for session "main"
    # Wait a bit for the connection to be returned to pool
    And we sleep 500ms
    # Create second session - it may get the same backend connection from pool
    And we create session "second" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    # Start a long-running query on second session
    And we send SimpleQuery "SELECT pg_sleep(2)" to session "second" without waiting for response
    # Wait a bit to ensure the query has started
    And we sleep 200ms
    # Send cancel request using main session's old backend_pid/secret_key
    # This should NOT cancel the second session's query because secret_key should be different
    And we send cancel request for session "main"
    # Wait for the pg_sleep to complete (3 seconds total to be safe)
    And we sleep 3000ms
    # Verify that second session completed without error (was not cancelled)
    Then session "second" should complete without error

  @cancel-race-multiple-reconnects
  Scenario: Multiple rapid reconnects should not allow cancel to kill wrong session
    # Create first session and store backend_pid/secret_key
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send SimpleQuery "SELECT 1" to session "first"
    And we abort TCP connection for session "first"
    And we sleep 100ms
    # Create second session, disconnect it too
    And we create session "second" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send SimpleQuery "SELECT 1" to session "second"
    And we abort TCP connection for session "second"
    And we sleep 100ms
    # Create third session, disconnect it too
    And we create session "third" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send SimpleQuery "SELECT 1" to session "third"
    And we abort TCP connection for session "third"
    And we sleep 100ms
    # Now create final session that will run a long query
    And we create session "final" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send SimpleQuery "SELECT pg_sleep(2)" to session "final" without waiting for response
    And we sleep 200ms
    # Try to cancel using all old session credentials - none should work
    And we send cancel request for session "first"
    And we send cancel request for session "second"
    And we send cancel request for session "third"
    # Wait for the pg_sleep to complete
    And we sleep 3000ms
    # Verify that final session completed without error
    Then session "final" should complete without error

  @cancel-race-same-backend-pid
  Scenario: Cancel with correct backend_pid but wrong secret_key should fail
    # Create first session and store backend_pid/secret_key
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    # Get the backend_pid from PostgreSQL
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    And we send SimpleQuery "SELECT 1" to session "first"
    # Disconnect first session
    And we abort TCP connection for session "first"
    And we sleep 500ms
    # Create second session - with pool_size=1 it should get the same backend connection
    And we create session "second" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    # Verify we got the same backend_pid (same PostgreSQL connection from pool)
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "second" and store backend_pid
    # Start a long-running query
    And we send SimpleQuery "SELECT pg_sleep(2)" to session "second" without waiting for response
    And we sleep 200ms
    # Send cancel using first session's credentials
    # Even if backend_pid is the same, secret_key should be different
    And we send cancel request for session "first"
    # Wait for the pg_sleep to complete
    And we sleep 3000ms
    # Verify that second session completed without error (cancel was rejected)
    Then session "second" should complete without error

  @cancel-race-interleaved
  Scenario: Interleaved connect/disconnect/cancel should not affect other sessions
    # Create session A and start a long query
    When we create session "A" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send SimpleQuery "SELECT pg_sleep(3)" to session "A" without waiting for response
    And we sleep 500ms
    # Try to cancel session A with correct credentials - this should work
    And we send cancel request for session "A"
    # Verify that session A received cancellation error
    Then session "A" should receive cancel error containing "canceling"
    # Now create session B, store credentials, then disconnect
    When we create session "B" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send SimpleQuery "SELECT 1" to session "B"
    And we abort TCP connection for session "B"
    And we sleep 100ms
    # Create session C and start a long query
    And we create session "C" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send SimpleQuery "SELECT pg_sleep(2)" to session "C" without waiting for response
    And we sleep 200ms
    # Try to cancel session B (which is already disconnected) - should not affect C
    And we send cancel request for session "B"
    And we sleep 3000ms
    # Verify that session C completed without error (cancel for B did not affect C)
    Then session "C" should complete without error
