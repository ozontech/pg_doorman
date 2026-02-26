@rust @rust-3 @min-pool-size
Feature: min_pool_size enforcement
  Test that min_pool_size setting is enforced at runtime.
  After each retain cycle, pg_doorman should replenish pools below min_pool_size.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @replenish-after-startup
  Scenario: Pool replenishes to min_pool_size after startup
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      retain_connections_time = 500
      server_lifetime = 60000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      min_pool_size = 3
      """
    # Wait for at least 2 retain cycles so replenish can create connections
    When we sleep for 1500 milliseconds
    # Check server connections via admin console
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 3

  @prewarm-at-startup
  Scenario: Pool is prewarmed at startup before retain cycle
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      retain_connections_time = 60000
      server_lifetime = 60000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      min_pool_size = 2
      """
    # Wait for prewarm to complete, but retain cycle (60s) has NOT fired yet
    When we sleep for 1000 milliseconds
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 2

  @maintain-after-expiry
  Scenario: Pool maintains min_pool_size after connections expire
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      retain_connections_time = 500
      server_lifetime = 1000
      server_idle_check_timeout = 0

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      min_pool_size = 2
      """
    # Create a session and make a query to establish at least 1 backend connection
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT pg_backend_pid()" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then we remember backend_pid from session "one" as "first_pid"
    # Wait for replenish to bring pool up to min_pool_size
    When we sleep for 2000 milliseconds
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 2
    # Wait for server_lifetime (1s ±20% jitter) to expire and retain to close + replenish
    When we sleep for 3000 milliseconds
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 2
    # Verify the backend was replaced (new PID after lifetime expiry)
    When we send Parse "" with query "SELECT pg_backend_pid()" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then we verify backend_pid from session "one" is different from "first_pid"
