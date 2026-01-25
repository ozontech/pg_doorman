@rust @rust-3 @copy-fail
Feature: COPY to non-existent table and session reuse
  Test that pg_doorman correctly handles COPY operation to non-existent table
  and that the backend connection is properly returned to the pool for reuse

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

  @copy-fail-nonexistent-table
  Scenario: COPY FROM STDIN to non-existent table fails and backend is reused
    # Session one: begin transaction
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "one"
    # Session one: remember backend_pid
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid
    # Session one: try COPY FROM STDIN to non-existent table - should fail immediately
    And we send CopyFromStdin "COPY nonexistent_table_xyz FROM STDIN" with data "1\ttest1\n" to session "one" expecting error
    # Check that session one received the error about non-existent table
    Then session "one" should receive error containing "nonexistent_table_xyz"
    # Session two: connect and verify it gets the same backend_pid (connection was returned to pool)
    When we sleep 100ms
    And we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "two" and store backend_pid
    Then backend_pid from session "one" should equal backend_pid from session "two"
    # Session one: send rollback (transaction was already aborted, but we send it anyway)
    When we send SimpleQuery "ROLLBACK" to session "one"
    # Session one: verify it still has the same backend_pid after rollback
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "one_after_rollback"
    Then backend_pid "one_after_rollback" from session "one" should equal initial backend_pid from session "one"
