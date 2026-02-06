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
    # Get initial backend PID
    And we send Parse "" with query "select pg_backend_pid()" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then we remember backend_pid from session "one" as "first_pid"
    # Wait for server_lifetime to expire
    When we sleep for 1000 milliseconds
    # Next transaction should get a new connection because old one exceeded lifetime
    And we send Parse "" with query "select pg_backend_pid()" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then we verify backend_pid from session "one" is different from "first_pid"

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
    # Create a connection and let it go idle
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select 1" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "1"
    # Wait for idle_check_timeout to trigger (>100ms)
    When we sleep for 200 milliseconds
    # Next query should succeed - alive check should pass
    And we send Parse "" with query "select 2" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
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
    # Create session and get backend PID
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "victim"
    # Create superuser session to terminate the backend
    When we create session "killer" to pg_doorman as "postgres" with password "" and database "example_db"
    # Terminate the backend - connection goes back to pool but is now dead
    And we terminate backend "victim" from session "one" via session "killer"
    # Wait for server_idle_check_timeout to trigger
    When we sleep for 200 milliseconds
    # Next query should succeed - check_alive should detect dead connection and get new one
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "new_pid"
    # Verify we got a different backend (the original was terminated)
    Then named backend_pid "new_pid" from session "one" is different from "victim"
