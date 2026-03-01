@rust @rust-4 @retain-max-quota
Feature: retain_connections_max quota enforcement across multiple pools
  When retain_connections_max is set, the total number of connections closed
  per retain cycle must not exceed the configured limit across ALL pools.

  Bug: when retain_connections_max > 0 and the global counter reaches the limit,
  remaining becomes 0 which is treated as "unlimited" by retain_oldest_first(),
  causing subsequent pools to lose ALL idle connections instead of none.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @retain-max-quota-two-pools
  Scenario: Two pools with expired connections respect retain_connections_max
    # Two pools each with 1 connection, server_lifetime=500ms.
    # retain_connections_max=1 means at most 1 connection should be closed per cycle.
    # retain_connections_time=3000ms ensures only 1 real retain cycle fires
    # before we check (immediate tick at t=0 finds nothing expired yet).
    # After lifetime expires and retain runs, at least 1 of 2 connections must survive.
    # BUG: quota exhaustion caused remaining=0 → unlimited closure → both closed in 1 cycle.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      server_lifetime = 500
      retain_connections_time = 3000
      retain_connections_max = 1
      idle_timeout = 60000
      server_idle_check_timeout = 0

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2

      [pools.example_db_2]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      server_database = "example_db"

      [[pools.example_db_2.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    # Establish 1 backend connection in each pool
    When we create session "pool_a" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT 1" to session "pool_a"
    And we send Bind "" to "" with params "" to session "pool_a"
    And we send Execute "" to session "pool_a"
    And we send Sync to session "pool_a"
    Then session "pool_a" should receive DataRow with "1"
    When we create session "pool_b" to pg_doorman as "example_user_1" with password "" and database "example_db_2"
    And we send Parse "" with query "SELECT 1" to session "pool_b"
    And we send Bind "" to "" with params "" to session "pool_b"
    And we send Execute "" to session "pool_b"
    And we send Sync to session "pool_b"
    Then session "pool_b" should receive DataRow with "1"
    # Both connections now idle. Wait for:
    # - server_lifetime (500ms ±20% jitter) to expire
    # - One retain cycle at t≈3000ms from doorman start to fire
    # The immediate tick at t≈0 finds nothing expired (connections just created).
    # At t≈3000ms the first real retain cycle closes at most 1 connection (quota=1).
    # We check at ~4000ms, before the next cycle at t≈6000ms.
    When we sleep for 4000 milliseconds
    # With retain_connections_max=1, only 1 retain cycle with expired connections has fired.
    # At most 1 connection should have been closed, so at least 1 must remain.
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 1
