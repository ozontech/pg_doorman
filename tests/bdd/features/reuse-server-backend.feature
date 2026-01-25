@rust @rust-3 @reuse-backend
Feature: Reuse server backend connection
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

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """

  @reuse-backend-1
  Scenario: Backend connection is reused after error
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "begin;" to session "one"
    And we send SimpleQuery "select pg_backend_pid()" to session "one" and store backend_pid
    And we send SimpleQuery "bad sql" to session "one"
    And we sleep 100ms
    And we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "select pg_backend_pid()" to session "two" and store backend_pid
    Then backend_pid from session "one" should equal backend_pid from session "two"

  @reuse-backend-2
  Scenario: Nested transactions with savepoint
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "begin;" to session "one"
    And we send SimpleQuery "savepoint sp;" to session "one"
    And we send SimpleQuery "select pg_backend_pid()" to session "one" and store backend_pid
    And we send SimpleQuery "bad sql;" to session "one"
    And we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "select pg_backend_pid()" to session "two" and store backend_pid
    Then backend_pid from session "one" should not equal backend_pid from session "two"
    When we send SimpleQuery "rollback to sp;" to session "one"
    And we send SimpleQuery "select pg_backend_pid()" to session "one" and store backend_pid as "one_after_rollback"
    Then backend_pid "one_after_rollback" from session "one" should equal initial backend_pid from session "one"
    When we send SimpleQuery "commit;" to session "one"
