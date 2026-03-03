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

  @stale-server-idle-in-transaction-timeout
  Scenario: Detect server killed by idle_in_transaction_session_timeout
    # PostgreSQL idle_in_transaction_session_timeout=2s kills backends
    # pg_doorman should detect the dead server via server_readable() and release the slot
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
    # Set idle_in_transaction_session_timeout on the database
    When we create session "setup" to postgres as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "ALTER DATABASE example_db SET idle_in_transaction_session_timeout = '2s'" to session "setup" without waiting
    And we sleep for 200 milliseconds
    # Client opens transaction through pg_doorman
    When we create session "client1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "BEGIN" to session "client1" without waiting
    Then we read SimpleQuery response from session "client1" within 2000ms
    When we send SimpleQuery "SELECT 1" to session "client1" without waiting
    Then we read SimpleQuery response from session "client1" within 2000ms
    # Wait for PostgreSQL to kill the backend (idle_in_transaction_session_timeout=2s)
    When we sleep for 3000 milliseconds
    # pg_doorman should have detected the dead server and released the slot
    # New client should be able to connect successfully
    When we create session "client2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "SELECT 2" to session "client2" without waiting
    Then we read SimpleQuery response from session "client2" within 5000ms
    Then session "client2" should receive DataRow with "2"
    # Reset the setting
    When we send SimpleQuery "ALTER DATABASE example_db RESET idle_in_transaction_session_timeout" to session "setup" without waiting

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

  @stale-server-client-idle-timeout
  Scenario: Client idle in transaction timeout releases pool slot
    # pg_doorman with client_idle_in_transaction_timeout=2000ms
    # Client opens transaction, then goes silent
    # pg_doorman should close by timeout and release the slot
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
      client_idle_in_transaction_timeout = 2000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    # Client opens transaction through pg_doorman
    When we create session "slow_client" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "BEGIN" to session "slow_client" without waiting
    Then we read SimpleQuery response from session "slow_client" within 2000ms
    When we send SimpleQuery "SELECT 1" to session "slow_client" without waiting
    Then we read SimpleQuery response from session "slow_client" within 2000ms
    # Wait for client_idle_in_transaction_timeout to fire (2s + margin)
    When we sleep for 3000 milliseconds
    # Pool slot should be released — new client should succeed
    When we create session "new_client" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "SELECT 2" to session "new_client" without waiting
    Then we read SimpleQuery response from session "new_client" within 5000ms
    Then session "new_client" should receive DataRow with "2"
