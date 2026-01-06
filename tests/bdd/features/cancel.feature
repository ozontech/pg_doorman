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

      [pools.example_db.users.0]
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
