@pool
Feature: Internal Pool.get benchmarks
  These benchmarks measure the internal Pool.get operation performance
  with real PostgreSQL connections.

  Scenario: Benchmark pool.get with single client and pool_size=1
    Given PostgreSQL started with options "-c max_connections=100" and pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    Given internal pool with size 1 and mode transaction
    When I benchmark pool.get with 10000000 iterations and save as "single_pool1"
    Then benchmark result "single_pool1" should exist
    And I print benchmark results to stdout

  Scenario: Benchmark pool.get with 1000 concurrent clients and pool_size=40
    Given PostgreSQL started with options "-c max_connections=100" and pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    Given internal pool with size 40 and mode transaction
    When I benchmark pool.get with 1000 concurrent clients and 10000 iterations per client and save as "concurrent_c1000_pool40"
    Then benchmark result "concurrent_c1000_pool40" should exist
    And I print benchmark results to stdout
