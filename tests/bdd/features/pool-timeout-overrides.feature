@rust @rust-4 @pool-timeout-override
Feature: Pool-level timeout overrides (server_lifetime, idle_timeout)
  Verify that pool-level overrides for server_lifetime and idle_timeout
  take effect instead of being silently ignored in favor of general settings.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @pool-override-server-lifetime
  Scenario: Pool-level server_lifetime override triggers recycle
    # General server_lifetime is 60s, but pool override is 500ms.
    # After ~1.5s the backend PID should change on next reuse (recycle).
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
      server_lifetime = 500

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "first_pid"
    # Wait longer than pool server_lifetime (500ms) but much less than general (60s)
    When we sleep for 1500 milliseconds
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "one" without waiting
    Then we read SimpleQuery response from session "one" within 5000ms
    Then we verify backend_pid from session "one" is different from "first_pid"

  @pool-override-idle-timeout
  Scenario: Pool-level idle_timeout override closes idle connections
    # General idle_timeout is 60s, but pool override is 500ms.
    # After the connection goes idle and retain runs, it should be closed.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      idle_timeout = 60000
      retain_connections_time = 200
      server_lifetime = 60000
      server_idle_check_timeout = 0

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      idle_timeout = 500

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "one" without waiting
    Then we read SimpleQuery response from session "one" within 2000ms
    # Let the connection go idle — pool idle_timeout=500ms should trigger
    When we sleep for 2000 milliseconds
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be 0

  @pool-override-two-pools-different-lifetime
  Scenario: Two pools with different pool-level server_lifetime
    # Pool A has server_lifetime=500ms, Pool B has server_lifetime=60s.
    # After 1.5s, Pool A should recycle (new PID), Pool B should keep same PID.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      server_lifetime = 30000
      server_idle_check_timeout = 0

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      server_lifetime = 500

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2

      [pools.example_db_2]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      server_database = "example_db"
      server_lifetime = 60000

      [[pools.example_db_2.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    When we create session "pool_a" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "pool_a" and store backend_pid as "pid_a"
    When we create session "pool_b" to pg_doorman as "example_user_1" with password "" and database "example_db_2"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "pool_b" and store backend_pid as "pid_b"
    # Wait longer than Pool A lifetime (500ms) but less than Pool B (60s)
    When we sleep for 1500 milliseconds
    # Pool A should recycle — different PID
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "pool_a" without waiting
    Then we read SimpleQuery response from session "pool_a" within 5000ms
    Then we verify backend_pid from session "pool_a" is different from "pid_a"
    # Pool B should keep the same PID — no recycle yet
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "pool_b" without waiting
    Then we read SimpleQuery response from session "pool_b" within 5000ms
    Then we verify backend_pid from session "pool_b" is same as "pid_b"

  @general-server-lifetime-baseline
  Scenario: General server_lifetime works correctly (baseline)
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
    When we sleep for 1500 milliseconds
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "one" without waiting
    Then we read SimpleQuery response from session "one" within 5000ms
    Then we verify backend_pid from session "one" is different from "first_pid"

  @general-idle-timeout-baseline
  Scenario: General idle_timeout works correctly (baseline)
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      idle_timeout = 500
      retain_connections_time = 200
      server_lifetime = 60000
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
    And we send SimpleQuery "SELECT 1" to session "one" without waiting
    Then we read SimpleQuery response from session "one" within 2000ms
    # Let the connection go idle — general idle_timeout=500ms should close it
    When we sleep for 2000 milliseconds
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be 0
