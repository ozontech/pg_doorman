@rust @rust-3 @deferred-begin
Feature: Deferred connection for standalone BEGIN
  Test that pg_doorman defers connection acquisition when client sends standalone BEGIN.
  This micro-optimization avoids reserving a server connection until the next query arrives,
  since BEGIN itself doesn't perform any actual server operations.

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
      pool_size = 10
      """

  @deferred-begin-no-backend
  Scenario: Standalone BEGIN does not acquire server backend
    # Create a client session
    When we create session "client1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Send BEGIN without waiting for response - this triggers the deferred connection optimization
    And we send SimpleQuery "begin;" to session "client1" without waiting for response
    # Small delay to ensure pg_doorman has processed the BEGIN message
    And we sleep 100ms
    # Check via admin console that no server backends are active
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pools" on admin session "admin" and store response
    # sv_active should be 0 because BEGIN is deferred - no server connection acquired yet
    Then admin session "admin" column "sv_active" should be between 0 and 0

  @deferred-begin-backend-killed
  Scenario: Client receives error when backend is killed after deferred BEGIN
    # This test verifies that when a server backend is terminated during a transaction
    # that started with deferred BEGIN, the client receives proper error notification.

    # Create main client session
    When we create session "main" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Send BEGIN - this is deferred, no server connection yet
    And we send SimpleQuery "begin;" to session "main"
    # Send a query to trigger actual connection and get backend PID
    And we send SimpleQuery "select pg_backend_pid()" to session "main" and store backend_pid

    # Create killer session to terminate the backend
    When we create session "killer" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Terminate the main session's backend using stored backend_pid
    And we terminate backend of session "main" via session "killer"

    # Small delay for termination to take effect
    And we sleep 100ms

    # Now try to execute a query on main session - should receive connection close or error
    When we send SimpleQuery "select 1" to session "main" expecting connection close
