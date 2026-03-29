@shared-pool-budget
Feature: Shared Pool Budget Controller

  Instance-level budget controller that limits total server connections
  to PostgreSQL across all pools. Each pool has guaranteed/weight/max.

  Background:
    Given a budget controller with max_connections=20 and min_lifetime=30s
    And pool "service_api" with guaranteed=5 weight=100 max=15
    And pool "batch_worker" with guaranteed=3 weight=50 max=10
    And pool "analytics" with guaranteed=0 weight=10 max=5

  # --- Normal operation ---

  Scenario: Guaranteed connections are granted immediately
    When "service_api" acquires 5 connections
    Then "service_api" has 5 held connections
    And total held is 5

  Scenario: Above-guarantee connections granted when pool not full
    When "service_api" acquires 8 connections
    Then "service_api" has 8 held connections
    And "service_api" has 3 above-guarantee connections

  Scenario: Multiple users fill pool without conflict
    When "service_api" acquires 8 connections
    And "batch_worker" acquires 5 connections
    And "analytics" acquires 3 connections
    Then total held is 16
    And 4 slots are free

  # --- EC-1: Equal weight, pool full ---

  Scenario: EC-1 Equal weight cannot evict, waits for return
    Given a budget controller with max_connections=20 and min_lifetime=30s
    And pool "user_a" with guaranteed=0 weight=100 max=20
    And pool "user_b" with guaranteed=0 weight=100 max=10
    When "user_a" acquires 20 connections
    Then "user_b" acquire would block
    When "user_a" releases 1 connection
    Then "user_b" is granted via schedule
    And "user_b" has 1 held connections
    And "user_a" has 19 held connections

  # --- EC-2: Lowest weight, pool full ---

  Scenario: EC-2 Lowest weight user waits, higher weight wins on release
    Given a budget controller with max_connections=10 and min_lifetime=0s
    And pool "high" with guaranteed=0 weight=100 max=10
    And pool "low" with guaranteed=0 weight=10 max=10
    And pool "filler" with guaranteed=0 weight=50 max=10
    When "filler" acquires 10 connections
    And "low" tries to acquire and would block
    And "high" tries to acquire and would block
    When "filler" releases 1 connection
    Then "high" is granted via schedule
    And "low" has 0 held connections

  # --- EC-3: Guaranteed evicts any weight ---

  Scenario: EC-3 Guaranteed request evicts above-guarantee regardless of weight
    Given all connections are 60 seconds old
    And "service_api" has 12 held connections
    And "batch_worker" has 5 held connections
    And "analytics" has 3 held connections
    When pool "admin" is registered with guaranteed=2 weight=1 max=2
    And "admin" acquires 1 connection
    Then the eviction was from "analytics"
    And "admin" has 1 held connections

  # --- EC-4: All connections within guarantee ---

  Scenario: EC-4 No above-guarantee connections to evict
    Given a budget controller with max_connections=8 and min_lifetime=30s
    And pool "svc" with guaranteed=5 weight=100 max=5
    And pool "batch" with guaranteed=3 weight=50 max=3
    When "svc" acquires 5 connections
    And "batch" acquires 3 connections
    And pool "analytics" is registered with guaranteed=0 weight=10 max=5
    Then "analytics" acquire would block

  # --- EC-5: Many dynamic users ---

  Scenario: EC-5 50 users with equal weight share pool round-robin
    Given a budget controller with max_connections=5 and min_lifetime=0s
    And 10 pools with guaranteed=0 weight=100 max=5
    When first 5 pools each acquire 1 connection
    Then total held is 5
    And pool 5 acquire would block
    When pool 0 releases 1 connection
    Then pool 5 is granted via schedule

  # --- EC-6: Guarantee budget overflow ---

  Scenario: EC-6 Guarantee overflow detected
    Given a budget controller with max_connections=10 and min_lifetime=30s
    And pool "a" with guaranteed=5 weight=100 max=10
    And pool "b" with guaranteed=3 weight=50 max=10
    Then guarantee validation passes
    When pool "c" is registered with guaranteed=5 weight=10 max=10
    Then guarantee validation fails with "sum(guaranteed)=13 > max_connections=10"

  # --- EC-7: min_lifetime=0 ---

  Scenario: EC-7 Zero min_lifetime allows immediate eviction
    Given a budget controller with max_connections=5 and min_lifetime=0s
    And pool "high" with guaranteed=0 weight=100 max=5
    And pool "low" with guaranteed=0 weight=10 max=5
    When "low" acquires 5 connections
    And "high" acquires 1 connection
    Then the eviction was from "low"
    And "low" has 4 held connections
    And "high" has 1 held connections

  # --- EC-8: Flap protection ---

  Scenario: EC-8 Young connections protected from eviction
    Given a budget controller with max_connections=5 and min_lifetime=30s
    And pool "high" with guaranteed=0 weight=100 max=5
    And pool "low" with guaranteed=0 weight=10 max=5
    When "low" acquires 5 connections
    Then "high" acquire would block
    And "low" has 5 held connections

  Scenario: EC-8 Aged connections become evictable
    Given a budget controller with max_connections=5 and min_lifetime=30s
    And pool "high" with guaranteed=0 weight=100 max=5
    And pool "low" with guaranteed=0 weight=10 max=5
    And "low" has 5 connections aged 60 seconds
    When "high" acquires 1 connection
    Then the eviction was from "low"
    And "low" has 4 held connections

  # --- Weight-based eviction order ---

  Scenario: Eviction targets lowest weight first
    Given a budget controller with max_connections=10 and min_lifetime=0s
    And pool "high" with guaranteed=0 weight=100 max=10
    And pool "mid" with guaranteed=0 weight=50 max=5
    And pool "low" with guaranteed=0 weight=10 max=5
    When "mid" acquires 5 connections
    And "low" acquires 5 connections
    And "high" acquires 1 connection
    Then the eviction was from "low"

  # --- Guaranteed connections are sacred ---

  Scenario: Guaranteed connections never evicted even by higher weight
    Given a budget controller with max_connections=5 and min_lifetime=0s
    And pool "high" with guaranteed=0 weight=100 max=5
    And pool "low" with guaranteed=5 weight=10 max=5
    When "low" acquires 5 connections
    Then "high" acquire would block

  # --- User at max ---

  Scenario: User at max_pool_size gets denied
    Given a budget controller with max_connections=100 and min_lifetime=0s
    And pool "user" with guaranteed=0 weight=100 max=3
    When "user" acquires 3 connections
    Then "user" acquire is denied with "user_max"

  # --- Tie-breaker: waiting count ---

  Scenario: Equal weight tie-broken by waiting count
    Given a budget controller with max_connections=1 and min_lifetime=0s
    And pool "a" with guaranteed=0 weight=100 max=5
    And pool "b" with guaranteed=0 weight=100 max=5
    When "a" acquires 1 connection
    And "b" tries to acquire 3 times and would block
    And "a" tries to acquire 1 time and would block
    When "a" releases 1 connection
    Then "b" is granted via schedule
