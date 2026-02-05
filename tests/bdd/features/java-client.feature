@java
Feature: Java client tests
  Test pg_doorman with Java PostgreSQL client (JDBC + HikariCP)

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      connect_timeout = 5000
      admin_username = "admin"
      admin_password = "admin"
      prepared_statements = true
      prepared_statements_cache_size = 10000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 40
      """

  Scenario: Run Java simple SELECT 1 test with HikariCP
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh simple_select simple_select.java
      """
    Then the command should succeed
    And the command output should contain "simple_select complete"
