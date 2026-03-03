@rust @rust-4 @stale-server-detection
Feature: Detect stale server connections during client idle in transaction
  When a client holds a server connection inside a transaction and the server
  terminates (e.g., idle_in_transaction_session_timeout, pg_terminate_backend),
  pg_doorman should detect this and release the pool slot.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @stale-server-pg-terminate-backend
  Scenario: Detect server killed by pg_terminate_backend
    # Client holds transaction, then backend is killed via pg_terminate_backend
    # pg_doorman should detect via server_readable() and release the slot
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      server_lifetime = 60000
      server_idle_check_timeout = 0

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1

      [[pools.example_db.users]]
      username = "postgres"
      password = ""
      pool_size = 2
      """
    # Client opens transaction and gets backend_pid
    When we create session "victim" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "victim" and store backend_pid as "victim_pid"
    When we send SimpleQuery "BEGIN" to session "victim" without waiting
    Then we read SimpleQuery response from session "victim" within 2000ms
    When we send SimpleQuery "SELECT 1" to session "victim" without waiting
    Then we read SimpleQuery response from session "victim" within 2000ms
    # Kill the backend through a separate superuser connection
    When we create session "killer" to pg_doorman as "postgres" with password "" and database "example_db"
    When we terminate backend "victim_pid" from session "victim" via session "killer"
    # Wait for pg_doorman to detect the dead server
    When we sleep for 500 milliseconds
    # Pool slot should be released — new client should succeed
    When we create session "client2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "SELECT 2" to session "client2" without waiting
    Then we read SimpleQuery response from session "client2" within 5000ms
    Then session "client2" should receive DataRow with "2"

