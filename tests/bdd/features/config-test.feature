@config-test
Feature: Configuration test mode (-t / --test-config)
  pg_doorman should support nginx-style configuration validation with -t flag.
  This allows validating configuration files before deployment without starting the server.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  Scenario: Test valid configuration file with -t flag
    Given pg_doorman started with config:
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
    # Stop pg_doorman to free the port, but keep config file
    When we sleep 500ms
    # Now test the config file with -t flag
    When I run shell command "${DOORMAN_BINARY} -t ${DOORMAN_CONFIG_FILE}"
    Then the command should succeed
    And the command output should contain "syntax is ok"
    And the command output should contain "test is successful"

  Scenario: Test invalid configuration file with -t flag
    When I run shell command "${DOORMAN_BINARY} -t /nonexistent/path/to/config.toml"
    Then the command should fail
    And the command output should contain "Config parse error"

  Scenario: Binary upgrade is cancelled when configuration is invalid
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    # Wait for pg_doorman to be fully ready
    When we sleep 1000ms
    # Store original PID for comparison
    And we store foreground pg_doorman PID as "original"
    # Overwrite config file with invalid content
    When we overwrite pg_doorman config file with invalid content:
      """
      this is not valid TOML syntax!!!
      [[[invalid
      """
    # Send SIGINT to trigger binary upgrade attempt
    When we send SIGINT to foreground pg_doorman
    # Wait a bit for config validation to run
    When we sleep 2000ms
    # Verify pg_doorman is still running (shutdown was cancelled)
    Then pg_doorman should still be running
    # Verify it's the same process (no binary upgrade happened)
    And foreground pg_doorman PID should be same as stored "original"
    # Verify we can still connect and execute queries
    When we create session "test" to pg_doorman as "example_user_1" with password "" and database "example_db"
    Then session "test" should be connected
    When we send SimpleQuery "SELECT 1" to session "test"
    When we close session "test"

  Scenario: Binary upgrade proceeds when configuration is valid
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    # Wait for pg_doorman to be fully ready
    When we sleep 1000ms
    # Store original PID for comparison
    And we store foreground pg_doorman PID as "original"
    # Config is still valid, so binary upgrade should proceed
    When we send SIGINT to foreground pg_doorman
    # Wait for binary upgrade to complete
    And we wait for foreground binary upgrade to complete
    # Verify service is still available (new process took over)
    Then foreground pg_doorman PID should be different from stored "original"
    # Verify we can still connect
    When we create session "after_upgrade" to pg_doorman as "example_user_1" with password "" and database "example_db"
    Then session "after_upgrade" should be connected
