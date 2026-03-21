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
  Scenario: Detect server killed by pg_terminate_backend — pool slot released
    # pool_size=1: if pg_doorman does not detect the dead server,
    # the slot stays occupied and client2 will hang forever.
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
    When we create session "victim" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "victim" and store backend_pid as "victim_pid"
    When we send SimpleQuery "BEGIN" to session "victim" without waiting
    Then we read SimpleQuery response from session "victim" within 2000ms
    When we send SimpleQuery "SELECT 1" to session "victim" without waiting
    Then we read SimpleQuery response from session "victim" within 2000ms
    # Kill the backend through a separate superuser connection
    When we create session "killer" to pg_doorman as "postgres" with password "" and database "example_db"
    When we terminate backend "victim_pid" from session "victim" via session "killer"
    # Wait for pg_doorman to detect the dead server (>100ms threshold + margin)
    When we sleep for 500 milliseconds
    # Pool slot should be released — new client should succeed
    When we create session "client2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "SELECT 2" to session "client2" without waiting
    Then we read SimpleQuery response from session "client2" within 5000ms
    Then session "client2" should receive DataRow with "2"

  @stale-server-victim-connection-closed
  Scenario: Victim client connection is closed after server is killed
    # pg_doorman detects the dead server, sends ErrorResponse to the victim,
    # and closes the connection. The victim's next query should fail.
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
    When we create session "victim" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "victim" and store backend_pid as "victim_pid"
    When we send SimpleQuery "BEGIN" to session "victim" without waiting
    Then we read SimpleQuery response from session "victim" within 2000ms
    When we send SimpleQuery "SELECT 1" to session "victim" without waiting
    Then we read SimpleQuery response from session "victim" within 2000ms
    # Kill the backend
    When we create session "killer" to pg_doorman as "postgres" with password "" and database "example_db"
    When we terminate backend "victim_pid" from session "victim" via session "killer"
    # Wait for detection
    When we sleep for 500 milliseconds
    # pg_doorman already closed the victim connection — next query must fail
    When we send SimpleQuery "SELECT 1" to session "victim" expecting connection close

  @stale-server-idle-in-transaction-timeout
  Scenario: Detect server killed by idle_in_transaction_session_timeout
    # PostgreSQL kills backends that are idle in transaction for too long.
    # We set the timeout to 1 second and verify pg_doorman detects it.
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
    # Set a short idle_in_transaction_session_timeout via superuser
    When we create session "setup" to pg_doorman as "postgres" with password "" and database "example_db"
    When we send SimpleQuery "ALTER SYSTEM SET idle_in_transaction_session_timeout = '1s'" to session "setup" without waiting
    Then we read SimpleQuery response from session "setup" within 2000ms
    When we send SimpleQuery "SELECT pg_reload_conf()" to session "setup" without waiting
    Then we read SimpleQuery response from session "setup" within 2000ms
    # Client opens transaction and goes idle
    When we create session "victim" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "BEGIN" to session "victim" without waiting
    Then we read SimpleQuery response from session "victim" within 2000ms
    When we send SimpleQuery "SELECT 1" to session "victim" without waiting
    Then we read SimpleQuery response from session "victim" within 2000ms
    # Wait for PostgreSQL to kill the backend (1s timeout + detection margin)
    When we sleep for 2000 milliseconds
    # Pool slot should be released — new client should succeed
    When we create session "client2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "SELECT 2" to session "client2" without waiting
    Then we read SimpleQuery response from session "client2" within 5000ms
    Then session "client2" should receive DataRow with "2"
    # Reset the setting
    When we send SimpleQuery "ALTER SYSTEM RESET idle_in_transaction_session_timeout" to session "setup" without waiting
    Then we read SimpleQuery response from session "setup" within 2000ms
    When we send SimpleQuery "SELECT pg_reload_conf()" to session "setup" without waiting
    Then we read SimpleQuery response from session "setup" within 2000ms

  @stale-server-fast-queries-no-false-positive
  Scenario: Fast queries in transaction do not trigger false detection
    # A client sending queries with small gaps (<100ms) must never hit
    # the server-monitoring code path — no false ServerDead detection.
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
      """
    When we create session "fast" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "BEGIN" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms
    # Rapid-fire 10 queries — none should fail
    When we send SimpleQuery "SELECT 1" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms
    When we send SimpleQuery "SELECT 2" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms
    Then session "fast" should receive DataRow with "2"
    When we send SimpleQuery "SELECT 3" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms
    Then session "fast" should receive DataRow with "3"
    When we send SimpleQuery "SELECT 4" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms
    Then session "fast" should receive DataRow with "4"
    When we send SimpleQuery "SELECT 5" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms
    Then session "fast" should receive DataRow with "5"
    When we send SimpleQuery "SELECT 6" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms
    Then session "fast" should receive DataRow with "6"
    When we send SimpleQuery "SELECT 7" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms
    Then session "fast" should receive DataRow with "7"
    When we send SimpleQuery "SELECT 8" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms
    Then session "fast" should receive DataRow with "8"
    When we send SimpleQuery "SELECT 9" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms
    Then session "fast" should receive DataRow with "9"
    When we send SimpleQuery "SELECT 10" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms
    Then session "fast" should receive DataRow with "10"
    # Transaction completes normally
    When we send SimpleQuery "COMMIT" to session "fast" without waiting
    Then we read SimpleQuery response from session "fast" within 2000ms

  @stale-server-multiple-kills-pool-recovers
  Scenario: Pool recovers after two consecutive backend kills
    # Kill backend twice in a row — pool must recover both times.
    # This verifies that detection + cleanup is not a one-shot mechanism.
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
    # First kill
    When we create session "victim1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "victim1" and store backend_pid as "pid1"
    When we send SimpleQuery "BEGIN" to session "victim1" without waiting
    Then we read SimpleQuery response from session "victim1" within 2000ms
    When we send SimpleQuery "SELECT 1" to session "victim1" without waiting
    Then we read SimpleQuery response from session "victim1" within 2000ms
    When we create session "killer" to pg_doorman as "postgres" with password "" and database "example_db"
    When we terminate backend "pid1" from session "victim1" via session "killer"
    When we sleep for 500 milliseconds
    # Pool recovered — second client can connect and start a transaction
    When we create session "victim2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "victim2" and store backend_pid as "pid2"
    When we send SimpleQuery "BEGIN" to session "victim2" without waiting
    Then we read SimpleQuery response from session "victim2" within 2000ms
    When we send SimpleQuery "SELECT 1" to session "victim2" without waiting
    Then we read SimpleQuery response from session "victim2" within 2000ms
    # Second kill
    When we terminate backend "pid2" from session "victim2" via session "killer"
    When we sleep for 500 milliseconds
    # Pool recovered again — third client works
    When we create session "client3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send SimpleQuery "SELECT 3" to session "client3" without waiting
    Then we read SimpleQuery response from session "client3" within 5000ms
    Then session "client3" should receive DataRow with "3"
