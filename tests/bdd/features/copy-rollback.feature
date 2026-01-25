@rust @rust-3 @copy-rollback
Feature: COPY with lock timeout and session reuse
  Test that pg_doorman correctly handles COPY operation that fails due to lock timeout
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

  @copy-rollback-lock-timeout
  Scenario: COPY FROM STDIN fails with lock timeout and backend is reused
    # Session one: create table if not exists
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TABLE IF NOT EXISTS copy_lock_test (id int, name text)" to session "one"
    # Session one: begin transaction
    And we send SimpleQuery "BEGIN" to session "one"
    # Session one: remember backend_pid
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid
    # Session one: set local lock_timeout to 1s
    And we send SimpleQuery "SET LOCAL lock_timeout TO '1s'" to session "one"
    # Session two: connect directly to PostgreSQL and lock the table
    When we create session "two" to postgres as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "two"
    And we send SimpleQuery "LOCK TABLE copy_lock_test IN ACCESS EXCLUSIVE MODE" to session "two"
    # Session one: try COPY FROM STDIN - should fail with lock timeout after ~1s
    # pg_doorman will automatically rollback the transaction and return connection to pool
    And we send CopyFromStdin "COPY copy_lock_test FROM STDIN" with data "1\ttest1\n" to session "one" expecting error
    # Session two: release the lock
    And we send SimpleQuery "COMMIT" to session "two"
    # Check that session one received the lock timeout error
    Then session "one" should receive error containing "lock"
    # Wait for connection to be returned to pool and create new session
    When we sleep 200ms
    And we create session "three" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "three" and store backend_pid
    Then backend_pid from session "one" should equal backend_pid from session "three"
