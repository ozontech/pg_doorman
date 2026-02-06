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

  Scenario: Run Java PBDE tests
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh pbde pbde.java
      """
    Then the command should succeed
    And the command output should contain "pbde complete"

  Scenario: Run Java prepared statements tests
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh prepared prepared.java
      """
    Then the command should succeed
    And the command output should contain "prepared complete"

  Scenario: Run Java batch tests
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh batch batch.java
      """
    Then the command should succeed
    And the command output should contain "batch 1 complete"
    And the command output should contain "batch 2 complete"

  Scenario: Run Java advanced prepared statements tests
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh prepared_advanced prepared_advanced.java
      """
    Then the command should succeed
    And the command output should contain "Test 1 complete"
    And the command output should contain "Test 2 complete"
    And the command output should contain "Test 3 complete"
    And the command output should contain "Test 4 complete"
    And the command output should contain "prepared_advanced complete"

  Scenario: Run Java error handling tests
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh errors errors.java
      """
    Then the command should succeed
    And the command output should contain "Test 1 complete"
    And the command output should contain "Test 2 complete"
    And the command output should contain "Test 3 complete"
    And the command output should contain "Test 4 complete"
    And the command output should contain "Test 5 complete"
    And the command output should contain "Test 6 complete"
    And the command output should contain "errors complete"

  Scenario: Run Java multi-session tests
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh multi_session multi_session.java
      """
    Then the command should succeed
    And the command output should contain "Test 1 complete"
    And the command output should contain "Test 2 complete"
    And the command output should contain "Test 3 complete"
    And the command output should contain "Test 4 complete"
    And the command output should contain "Test 5 complete"
    And the command output should contain "Test 6 complete"
    And the command output should contain "Test 7 complete"
    And the command output should contain "Test 8 complete"
    And the command output should contain "multi_session complete"

  Scenario: Run Java prepared statements with large data tests
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh prepared_extended_large prepared_extended_large.java
      """
    Then the command should succeed
    And the command output should contain "Test 1 complete"
    And the command output should contain "Test 2 complete"
    And the command output should contain "Test 3 complete"
    And the command output should contain "Test 4 complete"
    And the command output should contain "Test 5 complete"
    And the command output should contain "Test 6 complete"
    And the command output should contain "Test 7 complete"
    And the command output should contain "prepared_extended_large complete"

  Scenario: Run Java named prepared statements with Describe tests
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh prepared_named_describe prepared_named_describe.java
      """
    Then the command should succeed
    And the command output should contain "Test 1 complete"
    And the command output should contain "Test 2 complete"
    And the command output should contain "Test 3 complete"
    And the command output should contain "Test 4 complete"
    And the command output should contain "Test 5 complete"
    And the command output should contain "Test 6 complete"
    And the command output should contain "Test 7 complete"
    And the command output should contain "Test 8 complete"
    And the command output should contain "Test 9 complete"
    And the command output should contain "Test 10 complete"
    And the command output should contain "prepared_named_describe complete"

  Scenario: Run Java prepared statements stress test with Describe
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh prepared_stress_describe prepared_stress_describe.java
      """
    Then the command should succeed
    And the command output should contain "Test 1 complete"
    And the command output should contain "Test 2 complete"
    And the command output should contain "Test 3 complete"
    And the command output should contain "Test 4 complete"
    And the command output should contain "Test 5 complete"
    And the command output should contain "Test 6 complete"
    And the command output should contain "Test 7 complete"
    And the command output should contain "Test 8 complete"
    And the command output should contain "Test 9 complete"
    And the command output should contain "Test 10 complete"
    And the command output should contain "prepared_stress_describe complete"

  Scenario: Run Java Describe flow with cached prepared statements
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh describe_flow_cached describe_flow_cached.java
      """
    Then the command should succeed
    And the command output should contain "Test 1 complete"
    And the command output should contain "Test 2 complete"
    And the command output should contain "Test 3 complete"
    And the command output should contain "Test 4 complete"
    And the command output should contain "Test 5 complete"
    And the command output should contain "describe_flow_cached complete"

  Scenario: Run Java aggressive mixed tests (batch + prepared statements + extended protocol)
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      tests/java/run_test.sh aggressive_mixed aggressive_mixed.java
      """
    Then the command should succeed
    And the command output should contain "Test 1 complete"
    And the command output should contain "Test 2 complete"
    And the command output should contain "Test 3 complete"
    And the command output should contain "Test 4 complete"
    And the command output should contain "Test 5 complete"
    And the command output should contain "Test 6 complete"
    And the command output should contain "Test 7 complete"
    And the command output should contain "Test 8 complete"
    And the command output should contain "Test 9 complete"
    And the command output should contain "Test 10 complete"
    And the command output should contain "aggressive_mixed complete"
