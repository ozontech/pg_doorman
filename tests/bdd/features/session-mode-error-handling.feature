@rust @session-mode-error
Feature: Session mode does not destroy connections on SQL errors
  When pg_doorman runs in session mode and the server is in async protocol mode (Flush),
  a PostgreSQL ErrorResponse (syntax error, division by zero) should not mark the server
  connection as bad. The connection is still healthy and should be reused.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @session-error-parse
  Scenario: Session mode — Parse error via Flush does not destroy connection
    Given pg_doorman started with config:
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
      pool_mode = "session"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Record backend PID before error
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "before_error"
    # Trigger ErrorResponse in async mode: Parse with bad SQL + Flush
    And we send Parse "" with query "bad sql syntax" to session "s1"
    And we send Flush to session "s1"
    And we send Sync to session "s1"
    Then session "s1" should receive error containing "syntax"
    # Verify the connection is still alive — same backend PID
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "after_error"
    Then named backend_pid "after_error" from session "s1" is same as "before_error"

  @session-error-runtime
  Scenario: Session mode — runtime error (division by zero) via Flush does not destroy connection
    Given pg_doorman started with config:
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
      pool_mode = "session"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "before_error"
    # Trigger runtime error via extended protocol + Flush
    And we send Parse "" with query "SELECT 1/0" to session "s1"
    And we send Bind "" to "" with params "" to session "s1"
    And we send Execute "" to session "s1"
    And we send Flush to session "s1"
    And we send Sync to session "s1"
    Then session "s1" should receive error containing "division by zero"
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "after_error"
    Then named backend_pid "after_error" from session "s1" is same as "before_error"

  @session-error-reuse
  Scenario: Session mode — connection returned to pool after error, reused by next session
    Given pg_doorman started with config:
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
      pool_mode = "session"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    # First session: get PID, trigger error, close
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid
    And we send Parse "" with query "bad sql syntax" to session "s1"
    And we send Flush to session "s1"
    And we send Sync to session "s1"
    And we close session "s1"
    And we sleep 500ms
    # Second session: should get the same backend connection (not destroyed)
    When we create session "s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "s2" and store backend_pid
    Then backend_pid from session "s1" should equal backend_pid from session "s2"

  @session-error-multiple
  Scenario: Session mode — multiple sequential errors do not destroy connection
    Given pg_doorman started with config:
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
      pool_mode = "session"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "original"
    # Error 1: syntax error
    And we send Parse "" with query "not valid sql" to session "s1"
    And we send Flush to session "s1"
    And we send Sync to session "s1"
    # Error 2: division by zero
    And we send Parse "" with query "SELECT 1/0" to session "s1"
    And we send Bind "" to "" with params "" to session "s1"
    And we send Execute "" to session "s1"
    And we send Flush to session "s1"
    And we send Sync to session "s1"
    # Error 3: undefined table
    And we send Parse "" with query "SELECT * FROM nonexistent_table_xyz" to session "s1"
    And we send Flush to session "s1"
    And we send Sync to session "s1"
    # Connection should still be alive with the same backend
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "after_errors"
    Then named backend_pid "after_errors" from session "s1" is same as "original"

  @session-error-txmode-control
  Scenario: Transaction mode — Parse error via Flush destroys connection (control test)
    Given pg_doorman started with config:
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
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "before_error"
    # Trigger error in async mode — mark_bad is called in transaction mode
    And we send Parse "" with query "bad sql syntax" to session "s1"
    And we send Flush to session "s1"
    And we send Sync to session "s1"
    # Next query gets a new connection (old one was destroyed)
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "after_error"
    Then named backend_pid "after_error" from session "s1" is different from "before_error"
