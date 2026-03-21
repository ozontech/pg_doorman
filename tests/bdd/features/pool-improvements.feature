@rust @rust-3 @pool-improvements
Feature: Pool improvements - server_lifetime, idle check, oldest-first retention
  Test pool connection management improvements including:
  - server_lifetime enforcement for all connections (not just idle)
  - server_idle_check_timeout for detecting dead connections
  - oldest-first connection closure when retain_connections_max > 0

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @server-lifetime-active
  Scenario: Server lifetime is enforced for active connections during recycle
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      server_lifetime = 500
      server_idle_check_timeout = 0

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "first_pid"
    # Wait for server_lifetime to expire
    When we sleep for 1000 milliseconds
    # Next transaction should get a new connection because old one exceeded lifetime
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "new_pid_one"
    Then named backend_pid "new_pid_one" from session "one" is different from "first_pid"

  @idle-check-timeout
  Scenario: Idle connections are checked before reuse when server_idle_check_timeout is set
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      server_lifetime = 60000
      server_idle_check_timeout = 100
      connect_timeout = 1000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "one" without waiting
    Then we read SimpleQuery response from session "one" within 2000ms
    Then session "one" should receive DataRow with "1"
    # Wait for idle_check_timeout to trigger (>100ms)
    When we sleep for 200 milliseconds
    # Next query should succeed - alive check should pass
    When we send SimpleQuery "SELECT 2" to session "one" without waiting
    Then we read SimpleQuery response from session "one" within 2000ms
    Then session "one" should receive DataRow with "2"

  @terminate-backend-recovery
  Scenario: Dead connection is detected and replaced after pg_terminate_backend
    # Test that server_idle_check_timeout properly detects dead connections
    # when PostgreSQL terminates the backend (e.g., via pg_terminate_backend or restart)
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      server_lifetime = 60000
      server_idle_check_timeout = 100
      connect_timeout = 1000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2

      [[pools.example_db.users]]
      username = "postgres"
      password = ""
      pool_size = 2
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "victim"
    When we create session "killer" to pg_doorman as "postgres" with password "" and database "example_db"
    And we terminate backend "victim" from session "one" via session "killer"
    When we sleep for 200 milliseconds
    # Next query should succeed - check_alive should detect dead connection and get new one
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "new_pid"
    Then named backend_pid "new_pid" from session "one" is different from "victim"
