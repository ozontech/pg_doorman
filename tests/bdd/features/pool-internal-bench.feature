@bench @pool
Feature: Internal Pool.get benchmarks
  These benchmarks measure the internal Pool.get operation performance
  with real PostgreSQL connections. Used for testing assumptions before refactoring.
  Results are saved to ./documentations/docs/benchmark-internal-pool.md

  Scenario: Complete Pool.get benchmark suite
    Given PostgreSQL started with options "-c max_connections=200" and pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """

    # Single client benchmarks with different pool sizes
    Given internal pool with size 1 and mode transaction
    When I benchmark pool.get with 1000000 iterations and save as "single_pool1"
    Then benchmark result "single_pool1" should exist
    And I print internal pool benchmark results
    And pool status should show correct metrics

    Given internal pool with size 10 and mode transaction
    When I benchmark pool.get with 1000000 iterations and save as "single_pool10"
    Then benchmark result "single_pool10" should exist
    And I print internal pool benchmark results

    Given internal pool with size 50 and mode transaction
    When I benchmark pool.get with 1000000 iterations and save as "single_pool50"
    Then benchmark result "single_pool50" should exist
    And I print internal pool benchmark results

    # Concurrent client benchmarks
    Given internal pool with size 5 and mode transaction
    When I benchmark pool.get with 20 concurrent clients for 30 seconds and save as "concurrent_20c_5p"
    Then benchmark result "concurrent_20c_5p" should exist
    And I print internal pool benchmark results

    Given internal pool with size 10 and mode transaction
    When I benchmark pool.get with 50 concurrent clients for 30 seconds and save as "concurrent_50c_10p"
    Then benchmark result "concurrent_50c_10p" should exist
    And I print internal pool benchmark results

    Given internal pool with size 20 and mode transaction
    When I benchmark pool.get with 100 concurrent clients for 30 seconds and save as "concurrent_100c_20p"
    Then benchmark result "concurrent_100c_20p" should exist
    And I print internal pool benchmark results

    # Queue mode comparison
    Given internal pool with size 10 and queue mode fifo
    When I benchmark pool.get with 1000000 iterations and save as "fifo_pool10"
    Then benchmark result "fifo_pool10" should exist
    And I print internal pool benchmark results

    Given internal pool with size 10 and queue mode lifo
    When I benchmark pool.get with 1000000 iterations and save as "lifo_pool10"
    Then benchmark result "lifo_pool10" should exist
    And I print internal pool benchmark results

    # Save all results to markdown
    Then I save all benchmark results to markdown file
