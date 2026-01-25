@rust @rust-1 @batch-parse-describe-bug
Feature: Batch Parse/Describe bug reproduction
  Reproduces the bug where a batch containing:
  - A cached (skipped) Parse
  - A new Parse
  - Describe for the cached statement
  Results in an extra ParseComplete being sent to the client.

  The issue: When Parse is skipped (statement already cached), pg_doorman adds it to skipped_parses
  with target=ParameterDescription. Later, when processing Describe, it inserts ParseComplete
  before ParameterDescription. But if there's also a real Parse in the same batch, the client
  receives an extra ParseComplete, breaking the protocol.

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
      prepared_statements = true
      prepared_statements_cache_size = 10000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  @batch-bug-step1
  Scenario: Step 1 - First prepare statement stmt1 (will be cached)
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Describe "S" "stmt1" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-bug-step2
  Scenario: Step 2 - Reproduce the bug with batch containing cached Parse + new Parse + Describe
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Describe "S" "stmt1" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-bug-step3 @todo-skip
  Scenario: Step 3 - More complex batch with multiple cached and new statements
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Sync to both
    And we send Parse "stmt3" with query "select $1::bigint" to both
    And we send Describe "S" "stmt1" to both
    And we send Describe "S" "stmt2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-bug-step4
  Scenario: Step 4 - Disconnect and reconnect to test session persistence
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we disconnect from both
    And we reconnect to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Describe "S" "stmt2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-bug-step5
  Scenario: Step 5 - Combined test with disconnects (step1 + step2 + step3)
    # Step 1: First prepare statement stmt1 (will be cached)
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Describe "S" "stmt1" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Disconnect after step 1
    When we disconnect from both
    And we reconnect to both
    # Step 2: Reproduce the bug with batch containing cached Parse + new Parse + Describe
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Describe "S" "stmt1" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Disconnect after step 2
    When we disconnect from both
    And we reconnect to both
    # Step 3: More complex batch with multiple cached and new statements
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Sync to both
    And we send Parse "stmt3" with query "select $1::bigint" to both
    And we send Describe "S" "stmt1" to both
    And we send Describe "S" "stmt2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # EDGE CASES: Mixed batches with Parse/Bind/Execute in various orders
  # ============================================================================

  @batch-edge-case-2
  Scenario: New Parse followed by Bind/Execute for cached statement in same batch
    # First cache stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Now batch: new Parse + Bind to cached + Execute
    When we send Parse "stmt2" with query "select $1::text" to both
    And we send Bind "" to "stmt1" with params "42" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-3
  Scenario: Interleaved Parse/Bind/Execute for cached and new statements
    # First cache stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt3" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Complex interleaved batch
    When we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Bind "p1" to "stmt1" with params "100" to both
    And we send Bind "p2" to "stmt2" with params "world" to both
    And we send Execute "p1" to both
    And we send Execute "p2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-4
  Scenario: Multiple Describes for cached and new statements in single batch
    # First cache stmt1 and stmt2
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Batch with new Parse and multiple Describes
    When we send Parse "stmt3" with query "select $1::bigint" to both
    And we send Describe "S" "stmt1" to both
    And we send Describe "S" "stmt2" to both
    And we send Describe "S" "stmt3" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # EDGE CASES: Close operations in batches
  # ============================================================================

  @batch-edge-case-5
  Scenario: Close cached statement then re-Parse in same batch
    # First cache stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Close and re-Parse in same batch
    When we send Close "S" "stmt1" to both
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Describe "S" "stmt1" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-6
  Scenario: Close new statement immediately after Parse in same batch
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Close "S" "stmt1" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Describe "S" "stmt2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-7
  Scenario: Close portal between Execute operations
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Bind "p1" to "stmt1" with params "1" to both
    And we send Execute "p1" to both
    And we send Close "P" "p1" to both
    And we send Bind "p1" to "stmt1" with params "2" to both
    And we send Execute "p1" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # EDGE CASES: Unnamed statements with caching
  # ============================================================================

  @batch-edge-case-9
  Scenario: Multiple unnamed Parse overwrites in single batch
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select 1::int" to both
    And we send Parse "" with query "select 2::int" to both
    And we send Parse "" with query "select 3::int" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-10
  Scenario: Unnamed Parse with Describe then overwrite and Describe again
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int" to both
    And we send Describe "S" "" to both
    And we send Parse "" with query "select $1::text, $2::text" to both
    And we send Describe "S" "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # EDGE CASES: Describe Portal operations
  # ============================================================================

  @batch-edge-case-11
  Scenario: Describe Portal for cached statement in batch with new Parse
    # First cache stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Batch with new Parse and Describe Portal for cached
    When we send Parse "stmt2" with query "select $1::text" to both
    And we send Bind "portal1" to "stmt1" with params "42" to both
    And we send Describe "P" "portal1" to both
    And we send Execute "portal1" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-12
  Scenario: Multiple Describe Portal operations in single batch
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Bind "p2" to "stmt2" with params "hello" to both
    And we send Bind "p1" to "stmt1" with params "1" to both
    And we send Describe "P" "p2" to both
    And we send Describe "P" "p1" to both
    And we send Execute "p1" to both
    And we send Execute "p2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # EDGE CASES: Statement name reuse with different queries
  # ============================================================================

  @batch-edge-case-13
  Scenario: Redefine cached statement with different query in batch
    # First cache stmt1 with int query
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Close and redefine with different query type
    When we send Close "S" "stmt1" to both
    And we send Parse "stmt1" with query "select $1::text" to both
    And we send Bind "" to "stmt1" with params "hello" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # EDGE CASES: Flush operations in batches
  # ============================================================================

  @batch-edge-case-18
  Scenario: Multiple Flush operations with cached statements
    # First cache stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Multiple Flush
    When we send Parse "stmt1" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "stmt1" with params "1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Execute "" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # EDGE CASES: Session state after reconnect
  # ============================================================================

  @batch-edge-case-22
  Scenario: Complex batch after reconnect with server-side cached statements
    # First session - cache statements
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Disconnect
    When we disconnect from both
    And we reconnect to both
    # Complex batch - pg_doorman has cache, but client session is new
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt3" with query "select $1::bigint" to both
    And we send Describe "S" "stmt1" to both
    And we send Bind "" to "stmt1" with params "42" to both
    And we send Execute "" to both
    And we send Describe "S" "stmt3" to both
    And we send Bind "p2" to "stmt3" with params "999" to both
    And we send Execute "p2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-23
  Scenario: Multiple reconnects with statement reuse
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # First reconnect
    When we disconnect from both
    And we reconnect to both
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Second reconnect
    When we disconnect from both
    And we reconnect to both
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Parse "stmt3" with query "select $1::bigint" to both
    And we send Describe "S" "stmt1" to both
    And we send Describe "S" "stmt2" to both
    And we send Describe "S" "stmt3" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # STRESS TESTS: Large batches, disconnects, terminate, uneven stmt ordering
  # ============================================================================

  @batch-edge-case-24
  Scenario: Large batch with 20 Parse/Bind/Execute operations
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    # First cache some statements
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Disconnect to reset client session (pg_doorman keeps server cache)
    When we disconnect from both
    And we reconnect to both
    # Large batch with mix of cached and new statements
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s4" with query "select $1::float4" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Parse "s5" with query "select $1::float8" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Parse "s6" with query "select $1::bool" to both
    And we send Bind "p1" to "s1" with params "1" to both
    And we send Bind "p2" to "s2" with params "a" to both
    And we send Bind "p3" to "s3" with params "100" to both
    And we send Bind "p4" to "s4" with params "1.5" to both
    And we send Bind "p5" to "s5" with params "2.5" to both
    And we send Bind "p6" to "s6" with params "t" to both
    And we send Execute "p1" to both
    And we send Execute "p2" to both
    And we send Execute "p3" to both
    And we send Execute "p4" to both
    And we send Execute "p5" to both
    And we send Execute "p6" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-25
  Scenario: Uneven stmt ordering - cached statements scattered among new ones
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    # Cache stmt2 and stmt4 only
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Parse "stmt4" with query "select $1::float8" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Disconnect to reset client session
    When we disconnect from both
    And we reconnect to both
    # Batch with uneven ordering: new, cached, new, cached, new
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Parse "stmt3" with query "select $1::bigint" to both
    And we send Parse "stmt4" with query "select $1::float8" to both
    And we send Parse "stmt5" with query "select $1::bool" to both
    And we send Describe "S" "stmt1" to both
    And we send Describe "S" "stmt2" to both
    And we send Describe "S" "stmt3" to both
    And we send Describe "S" "stmt4" to both
    And we send Describe "S" "stmt5" to both
    And we send Bind "" to "stmt3" with params "999" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-26
  Scenario: Multiple disconnects with increasing batch complexity
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # First disconnect - simple batch
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Bind "" to "s1" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Second disconnect - medium batch
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Describe "S" "s1" to both
    And we send Bind "p1" to "s2" with params "hello" to both
    And we send Execute "p1" to both
    And we send Describe "S" "s3" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Third disconnect - complex batch
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Parse "s4" with query "select $1::float4" to both
    And we send Describe "S" "s1" to both
    And we send Describe "S" "s2" to both
    And we send Bind "p1" to "s3" with params "100" to both
    And we send Bind "p2" to "s4" with params "1.5" to both
    And we send Execute "p1" to both
    And we send Execute "p2" to both
    And we send Describe "S" "s3" to both
    And we send Describe "S" "s4" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-27
  Scenario: Batch with reversed statement order after reconnect
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    # Cache in order: s1, s2, s3, s4, s5
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Parse "s4" with query "select $1::float4" to both
    And we send Parse "s5" with query "select $1::float8" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Disconnect and use in reverse order
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s5" with query "select $1::float8" to both
    And we send Parse "s4" with query "select $1::float4" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Describe "S" "s5" to both
    And we send Describe "S" "s4" to both
    And we send Describe "S" "s3" to both
    And we send Describe "S" "s2" to both
    And we send Describe "S" "s1" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-28
  Scenario: Interleaved Parse/Describe/Bind/Execute with multiple reconnects
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # First reconnect - interleaved operations
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Describe "S" "s1" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Describe "S" "s2" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Describe "S" "s3" to both
    And we send Bind "p1" to "s1" with params "1" to both
    And we send Bind "p2" to "s2" with params "a" to both
    And we send Bind "p3" to "s3" with params "100" to both
    And we send Execute "p1" to both
    And we send Execute "p2" to both
    And we send Execute "p3" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Second reconnect - more interleaving
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Bind "p1" to "s1" with params "10" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Execute "p1" to both
    And we send Bind "p2" to "s2" with params "b" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Execute "p2" to both
    And we send Bind "p3" to "s3" with params "200" to both
    And we send Execute "p3" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-29
  Scenario: Close and re-create statements after multiple reconnects
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # First reconnect - close s1, keep s2
    When we disconnect from both
    And we reconnect to both
    And we send Close "S" "s1" to both
    And we send Parse "s1" with query "select $1::bigint" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Describe "S" "s1" to both
    And we send Describe "S" "s2" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Second reconnect - close s2, keep s1
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::bigint" to both
    And we send Close "S" "s2" to both
    And we send Parse "s2" with query "select $1::float4" to both
    And we send Describe "S" "s1" to both
    And we send Describe "S" "s2" to both
    And we send Bind "" to "s1" with params "999" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-31
  Scenario: Stress test with 5 reconnects and growing statement pool
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Reconnect 1 - add s2
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Reconnect 2 - add s3
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Reconnect 3 - add s4
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Parse "s4" with query "select $1::float4" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Reconnect 4 - add s5
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Parse "s4" with query "select $1::float4" to both
    And we send Parse "s5" with query "select $1::float8" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Reconnect 5 - use all with Describe
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Parse "s4" with query "select $1::float4" to both
    And we send Parse "s5" with query "select $1::float8" to both
    And we send Describe "S" "s1" to both
    And we send Describe "S" "s2" to both
    And we send Describe "S" "s3" to both
    And we send Describe "S" "s4" to both
    And we send Describe "S" "s5" to both
    And we send Bind "p1" to "s1" with params "1" to both
    And we send Bind "p2" to "s2" with params "a" to both
    And we send Bind "p3" to "s3" with params "100" to both
    And we send Bind "p4" to "s4" with params "1.5" to both
    And we send Bind "p5" to "s5" with params "2.5" to both
    And we send Execute "p1" to both
    And we send Execute "p2" to both
    And we send Execute "p3" to both
    And we send Execute "p4" to both
    And we send Execute "p5" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-32
  Scenario: Batch with Flush between Parse operations after reconnect
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Reconnect and use Flush between operations
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "p1" to "s1" with params "1" to both
    And we send Bind "p2" to "s2" with params "a" to both
    And we send Bind "p3" to "s3" with params "100" to both
    And we send Execute "p1" to both
    And we send Execute "p2" to both
    And we send Execute "p3" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-33
  Scenario: Complex batch with portal operations and reconnects
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Reconnect - complex portal operations
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Parse "s3" with query "select $1::bigint" to both
    And we send Bind "portal_a" to "s1" with params "1" to both
    And we send Bind "portal_b" to "s2" with params "hello" to both
    And we send Bind "portal_c" to "s3" with params "999" to both
    And we send Describe "P" "portal_a" to both
    And we send Describe "P" "portal_b" to both
    And we send Describe "P" "portal_c" to both
    And we send Execute "portal_a" to both
    And we send Close "P" "portal_a" to both
    And we send Execute "portal_b" to both
    And we send Close "P" "portal_b" to both
    And we send Execute "portal_c" to both
    And we send Close "P" "portal_c" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Second reconnect - reuse same portal names
    When we disconnect from both
    And we reconnect to both
    And we send Parse "s1" with query "select $1::int" to both
    And we send Parse "s2" with query "select $1::text" to both
    And we send Bind "portal_a" to "s1" with params "10" to both
    And we send Bind "portal_b" to "s2" with params "world" to both
    And we send Execute "portal_a" to both
    And we send Execute "portal_b" to both
    And we send Sync to both
    Then we should receive identical messages from both
