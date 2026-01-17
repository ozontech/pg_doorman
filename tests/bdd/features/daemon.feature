@rust @daemon
Feature: Daemon mode with PID file synchronization
  pg_doorman should properly daemonize and write PID file before parent exits.
  This ensures proper integration with supervisor programs that rely on PID files.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  Scenario: PID file is written before daemonize returns
    Given pg_doorman started in daemon mode with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      daemon_pid_file = "/tmp/pg_doorman_pid_scenario_1"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    Then PID file "/tmp/pg_doorman_pid_scenario_1" should contain running daemon PID

  Scenario: Daemon responds to connections after daemonization
    Given pg_doorman started in daemon mode with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      daemon_pid_file = "/tmp/pg_doorman_pid_scenario_2"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    When we create session "test" to pg_doorman as "example_user_1" with password "" and database "example_db"
    Then PID file "/tmp/pg_doorman_pid_scenario_2" should contain running daemon PID

  Scenario: Graceful reload changes PID and new sessions work
    Given pg_doorman started in daemon mode with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      daemon_pid_file = "/tmp/pg_doorman_pid_scenario_3"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    # Wait for daemon to be fully ready
    When we sleep 2000ms
    # Open session and verify it works
    And we create session "before_reload" to pg_doorman as "example_user_1" with password "" and database "example_db"
    Then session "before_reload" should be connected
    # Close session before reload
    When we close session "before_reload"
    # Store original PID for comparison
    And we store daemon PID from "/tmp/pg_doorman_pid_scenario_3" as "original"
    # Send SIGINT to trigger binary-upgrade
    And we send SIGINT to daemon from PID file "/tmp/pg_doorman_pid_scenario_3"
    # Wait for new daemon to start
    And we sleep 2000ms
    # Verify PID has changed
    Then PID file "/tmp/pg_doorman_pid_scenario_3" should contain different PID than stored "original"
    And PID file "/tmp/pg_doorman_pid_scenario_3" should contain running daemon PID
    # Wait for PostgreSQL to recover from old daemon's graceful shutdown
    When we sleep 1500ms
    # Open new session and verify it works
    And we create session "after_reload" to pg_doorman as "example_user_1" with password "" and database "example_db"
    Then session "after_reload" should be connected

  Scenario: Graceful shutdown allows active transactions to complete but rejects new ones
    Given pg_doorman started in daemon mode with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      daemon_pid_file = "/tmp/pg_doorman_pid_scenario_4"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    # Wait for daemon to be fully ready
    When we sleep 2000ms
    # Open session and start a transaction
    And we create session "original" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "original"
    # Store backend PID for comparison
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "original" and store backend_pid as "original_backend"
    # Send SIGINT to trigger graceful shutdown
    And we send SIGINT to daemon from PID file "/tmp/pg_doorman_pid_scenario_4"
    # Wait for new daemon to start
    And we sleep 2000ms
    # Open new session - should connect to new daemon
    And we create session "new" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "new" and store backend_pid as "new_backend"
    # Verify new session has different backend PID
    Then stored PID "new_backend" should be different from "original_backend"
    # Original session can still execute queries in active transaction
    When we send SimpleQuery "SELECT 1" to session "original"
    # Commit the transaction - after this the connection should be closed by pooler
    And we send SimpleQuery "COMMIT" to session "original"
    # Close new session
    When we close session "new"
