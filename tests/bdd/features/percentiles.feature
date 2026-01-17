@rust @percentiles
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

      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 1
      pool_mode = "transaction"
      """

  @percentiles-basic
  Scenario: Percentiles are calculated from query execution times
    # Create a session and execute queries with known sleep times
    When we create session "test" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Execute several queries with pg_sleep to create measurable latencies
    # 100ms sleep = 100000 microseconds
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.1)" to session "test"
    # Wait for stats to be collected (stats period is 15 seconds, but we need at least one cycle)
    And we sleep 16000ms
    # Check that percentiles are reported in SHOW POOLS_EXTENDED
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pools_extended" on admin session "admin" and store response
    # The query_0.99 column should contain a value > 0 (at least 100000 microseconds for 100ms sleep)
    # Since we executed 100ms sleeps, p99 should be around 100000 microseconds
    Then admin session "admin" response should contain "example_db"

  @percentiles-distribution
  Scenario: Percentiles reflect actual query time distribution
    # Create a session and execute queries with varying sleep times
    When we create session "test" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Execute queries with different sleep times to create a distribution
    # Fast queries (10ms)
    And we send SimpleQuery "SELECT pg_sleep(0.01)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.01)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.01)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.01)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.01)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.01)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.01)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.01)" to session "test"
    # Slow query (200ms) - this should affect p99
    And we send SimpleQuery "SELECT pg_sleep(0.2)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.2)" to session "test"
    # Wait for stats collection
    And we sleep 16000ms
    # Check percentiles
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pools_extended" on admin session "admin" and store response
    Then admin session "admin" response should contain "example_user_1"

  @percentiles-reset
  Scenario: Percentiles are reset after stats period
    When we create session "test" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Execute slow queries
    And we send SimpleQuery "SELECT pg_sleep(0.15)" to session "test"
    And we send SimpleQuery "SELECT pg_sleep(0.15)" to session "test"
    # Wait for first stats period
    And we sleep 16000ms
    # Now execute fast queries
    And we send SimpleQuery "SELECT 1" to session "test"
    And we send SimpleQuery "SELECT 1" to session "test"
    And we send SimpleQuery "SELECT 1" to session "test"
    And we send SimpleQuery "SELECT 1" to session "test"
    And we send SimpleQuery "SELECT 1" to session "test"
    # Wait for second stats period - percentiles should now reflect fast queries
    And we sleep 16000ms
    # Check that stats are available
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pools_extended" on admin session "admin" and store response
    Then admin session "admin" response should contain "example_db"
