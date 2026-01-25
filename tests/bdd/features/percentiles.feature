@rust @rust-3 @percentiles
Feature: HDR Histogram percentiles for query and transaction times
  Test that pg_doorman correctly calculates percentiles using HDR histograms
  and reports them via SHOW POOLS_EXTENDED command

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      pool_mode = "transaction"
      """

  @percentiles-values
  Scenario: Percentiles reflect actual query time distribution with correct values
    # Create a session and execute queries with known sleep times
    # Distribution: 8x 100ms (fast) + 2x 200ms (slow)
    # Expected: p50 ≈ 100ms (100000us), p99 ≈ 200ms (200000us)
    When we create session "test" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Fast queries (100ms each)
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    # Slow queries (200ms each) - these should affect p99
    And we send SimpleQuery "SELECT pg_sleep(0.2)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.2)" to session "test"
    # Check percentiles immediately (before stats period resets histograms)
    # Percentiles are calculated from HDR histogram which is updated in real-time
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pools_extended" on admin session "admin" and store response
    # Verify response contains our pool
    Then admin session "admin" response should contain "example_db"
    # p99 (query_0.99) should be around 200ms = 200000 microseconds (allow 150000-300000 range)
    And admin session "admin" column "query_0.99" should be between 150000 and 300000
    # p50 (query_0.5) should be around 100ms = 100000 microseconds (allow 80000-180000 range)
    And admin session "admin" column "query_0.5" should be between 80000 and 180000

  @percentiles-different-times
  Scenario: Percentiles correctly distinguish between fast and slow queries
    When we create session "test" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Execute queries with different sleep times: 50ms and 150ms
    # 9x 50ms (fast) + 1x 150ms (slow)
    And we send SimpleQuery "SELECT pg_sleep(0.05)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.05)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.05)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.05)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.05)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.05)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.05)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.05)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.05)" to session "test"
    # One slow query
    And we send SimpleQuery "SELECT pg_sleep(0.15)" to session "test"
    # Check percentiles
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pools_extended" on admin session "admin" and store response
    Then admin session "admin" response should contain "example_db"
    # p50 should be around 50ms = 50000 microseconds (allow 30000-80000 range)
    And admin session "admin" column "query_0.5" should be between 30000 and 80000
    # p99 should be around 150ms = 150000 microseconds (allow 100000-200000 range)
    And admin session "admin" column "query_0.99" should be between 100000 and 200000
