@rust @batch-parse-describe-bug
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

  @batch-edge-case-3 @todo-skip
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

  @batch-edge-case-4 @todo-skip
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

  @batch-edge-case-5 @todo-skip
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

  @batch-edge-case-6 @todo-skip
  Scenario: Close new statement immediately after Parse in same batch
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Close "S" "stmt1" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Describe "S" "stmt2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-7 @todo-skip
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

  @batch-edge-case-8 @todo-skip
  Scenario: Unnamed Parse overwrite with cached named Parse in batch
    # First cache named stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Batch with unnamed Parse and cached named Parse
    When we send Parse "" with query "select $1::text" to both
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Bind "" to "" with params "hello" to both
    And we send Execute "" to both
    And we send Bind "p2" to "stmt1" with params "42" to both
    And we send Execute "p2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-9 @todo-skip
  Scenario: Multiple unnamed Parse overwrites in single batch
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select 1::int" to both
    And we send Parse "" with query "select 2::int" to both
    And we send Parse "" with query "select 3::int" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-10 @todo-skip
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

  @batch-edge-case-11 @todo-skip
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

  @batch-edge-case-12 @todo-skip
  Scenario: Multiple Describe Portal operations in single batch
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Bind "p1" to "stmt1" with params "1" to both
    And we send Bind "p2" to "stmt2" with params "hello" to both
    And we send Describe "P" "p1" to both
    And we send Describe "P" "p2" to both
    And we send Execute "p1" to both
    And we send Execute "p2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # EDGE CASES: Statement name reuse with different queries
  # ============================================================================

  @batch-edge-case-13 @todo-skip
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

  @batch-edge-case-14 @todo-skip
  Scenario: Parse same name twice in batch (second should fail or override)
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt1" with query "select $1::text" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # EDGE CASES: Complex multi-statement batches
  # ============================================================================

  @batch-edge-case-15 @todo-skip
  Scenario: Large batch with mixed cached and new statements
    # First cache multiple statements
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "cached1" with query "select $1::int" to both
    And we send Parse "cached2" with query "select $1::text" to both
    And we send Parse "cached3" with query "select $1::bigint" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Large mixed batch
    When we send Parse "cached1" with query "select $1::int" to both
    And we send Parse "new1" with query "select $1::float4" to both
    And we send Parse "cached2" with query "select $1::text" to both
    And we send Parse "new2" with query "select $1::float8" to both
    And we send Describe "S" "cached1" to both
    And we send Describe "S" "new1" to both
    And we send Describe "S" "cached2" to both
    And we send Describe "S" "new2" to both
    And we send Bind "" to "cached3" with params "999" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-16 @todo-skip
  Scenario: Alternating cached/new Parse with Describes
    # First cache stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Alternating pattern
    When we send Parse "stmt1" with query "select $1::int" to both
    And we send Describe "S" "stmt1" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Describe "S" "stmt2" to both
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Describe "S" "stmt1" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # EDGE CASES: Flush operations in batches
  # ============================================================================

  @batch-edge-case-17 @todo-skip
  Scenario: Flush between cached Parse and new Parse
    # First cache stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Batch with Flush
    When we send Parse "stmt1" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Describe "S" "stmt1" to both
    And we send Describe "S" "stmt2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-18 @todo-skip
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
  # EDGE CASES: Error handling in batches
  # ============================================================================

  @batch-edge-case-19 @todo-skip
  Scenario: Describe non-existent statement after cached Parse
    # First cache stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Try to describe non-existent statement
    When we send Parse "stmt1" with query "select $1::int" to both
    And we send Describe "S" "nonexistent" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-20 @todo-skip
  Scenario: Bind to non-existent statement after cached Parse
    # First cache stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Try to bind to non-existent statement
    When we send Parse "stmt1" with query "select $1::int" to both
    And we send Bind "" to "nonexistent" with params "1" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # EDGE CASES: Session state after reconnect
  # ============================================================================

  @batch-edge-case-22 @todo-skip
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
  # EDGE CASES: Empty and edge parameter cases
  # ============================================================================

  @batch-edge-case-24 @todo-skip
  Scenario: Cached Parse with empty portal name operations
    # First cache stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Operations with empty portal
    When we send Parse "stmt1" with query "select $1::int" to both
    And we send Bind "" to "stmt1" with params "1" to both
    And we send Describe "P" "" to both
    And we send Execute "" to both
    And we send Close "P" "" to both
    And we send Bind "" to "stmt1" with params "2" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-edge-case-25 @todo-skip
  Scenario: Mixed named and unnamed portals with cached statements
    # First cache stmt1
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Mixed portal operations
    When we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Bind "" to "stmt1" with params "1" to both
    And we send Bind "named_portal" to "stmt2" with params "hello" to both
    And we send Execute "" to both
    And we send Execute "named_portal" to both
    And we send Bind "" to "stmt2" with params "world" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both