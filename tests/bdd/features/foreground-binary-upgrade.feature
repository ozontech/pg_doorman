@rust @foreground-upgrade
Feature: Foreground mode binary upgrade
  pg_doorman should support binary upgrade in foreground mode by passing
  the listener socket to the new process via --inherit-fd argument.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  Scenario: Binary upgrade in foreground mode maintains service availability
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
      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    # Wait for pg_doorman to be fully ready
    When we sleep 1000ms
    # Open session and verify it works
    And we create session "before_upgrade" to pg_doorman as "example_user_1" with password "" and database "example_db"
    Then session "before_upgrade" should be connected
    # Close session before upgrade
    When we close session "before_upgrade"
    # Store original PID for comparison
    And we store foreground pg_doorman PID as "original"
    # Send SIGINT to trigger binary upgrade
    And we send SIGINT to foreground pg_doorman
    # Wait for binary upgrade to complete
    And we wait for foreground binary upgrade to complete
    # Verify service is still available
    Then foreground pg_doorman PID should be different from stored "original"
    # Open new session and verify it works with new process
    When we create session "after_upgrade" to pg_doorman as "example_user_1" with password "" and database "example_db"
    Then session "after_upgrade" should be connected

  Scenario: Binary upgrade preserves active transactions during graceful shutdown
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
      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    # Wait for pg_doorman to be fully ready
    When we sleep 1000ms
    # Open session and start a transaction
    And we create session "original" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "original"
    # Store backend PID for comparison
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "original" and store backend_pid as "original_backend"
    # Send SIGINT to trigger binary upgrade
    And we send SIGINT to foreground pg_doorman
    # Wait for new process to start
    And we wait for foreground binary upgrade to complete
    # Open new session - should connect to new process
    And we create session "new" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "new" and store backend_pid as "new_backend"
    # Verify new session has different backend PID (connected through new process)
    Then stored PID "new_backend" should be different from "original_backend"
    # Original session can still execute queries in active transaction
    When we send SimpleQuery "SELECT 1" to session "original"
    # Commit the transaction
    And we send SimpleQuery "COMMIT" to session "original"
    # Close sessions
    When we close session "new"
