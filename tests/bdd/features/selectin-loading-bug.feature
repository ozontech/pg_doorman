@selectin
Feature: Selectin loading bug reproduction (asyncpg + SQLAlchemy)
  Reproduces protocol desync when pg_doorman's prepared statement cache eviction
  happens while async_mode=true (between Flush and Sync).

  The bug: register_prepared_statement() sends Close+Sync for evicted statement,
  but recv() immediately breaks (async_mode=true, expected_responses=0).
  This leaves CloseComplete+ReadyForQuery in TCP buffer, causing protocol desync.

  Trigger conditions:
  1. Small prepared_statements_cache_size (forces eviction)
  2. Parse after Flush without Sync in between (async_mode stays true)
  3. Parse triggers eviction from server cache

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
      prepared_statements_cache_size = 2

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  # ============================================================================
  # CORE BUG: Cache eviction during async_mode=true causes protocol desync
  # ============================================================================

  @selectin-protocol @selectin-eviction
  Scenario: cache eviction during async_mode=true causes protocol desync
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    # 1st Parse+Describe+Flush: fills cache slot 1, sets async_mode=true
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Describe "S" "stmt1" to both
    And we send Flush to both
    And we verify partial response received from both
    # async_mode=true, expected_responses=0, server NOT released (async prevents it)
    # 2nd Parse+Describe+Flush: fills cache slot 2, still async_mode=true
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Describe "S" "stmt2" to both
    And we send Flush to both
    And we verify partial response received from both
    # 3rd Parse: cache is full (size=2), EVICTS oldest entry!
    # register_prepared_statement sends Close+Sync but recv fails (async_mode=true, expected=0)
    # CloseComplete+ReadyForQuery left in TCP buffer → PROTOCOL DESYNC
    And we send Parse "stmt3" with query "select $1::bigint" to both
    And we send Describe "S" "stmt3" to both
    And we send Flush to both
    And we verify partial response received from both
    # Now execute all three
    And we send Bind "" to "stmt1" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Bind "" to "stmt2" with params "hello" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Bind "" to "stmt3" with params "100" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @selectin-protocol @selectin-eviction
  Scenario: cache eviction with 4 pipelined prepares (deeper desync)
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    # Pipeline 4 Parse+Describe+Flush (cache_size=2 → 2 evictions)
    And we send Parse "p1" with query "select $1::int" to both
    And we send Describe "S" "p1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Parse "p2" with query "select $1::text" to both
    And we send Describe "S" "p2" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Parse "p3" with query "select $1::bigint" to both
    And we send Describe "S" "p3" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Parse "p4" with query "select $1::float4" to both
    And we send Describe "S" "p4" to both
    And we send Flush to both
    And we verify partial response received from both
    # Execute all four
    And we send Bind "" to "p1" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Bind "" to "p2" with params "hello" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Bind "" to "p3" with params "100" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Bind "" to "p4" with params "1.5" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # BASIC TESTS: sequential Parse+Flush → Bind+Sync (should still work)
  # With cache_size=2, eviction happens but async_mode=false during Parse
  # ============================================================================

  @selectin-protocol @selectin-basic
  Scenario: sequential Parse+Flush → Bind+Sync works even with small cache
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    # Each cycle: Parse+Describe+Flush → Bind+Execute+Sync
    # Sync resets async_mode=false, so eviction during next Parse is safe
    And we send Parse "seq1" with query "select $1::int" to both
    And we send Describe "S" "seq1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "seq1" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Parse "seq2" with query "select $1::text" to both
    And we send Describe "S" "seq2" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "seq2" with params "hello" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Parse "seq3" with query "select $1::bigint" to both
    And we send Describe "S" "seq3" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "seq3" with params "100" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @selectin-protocol @selectin-reconnect
  Scenario: reconnect reuses cached statements with small cache
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    # First session: 3 sequential cycles
    And we send Parse "s1_q1" with query "select $1::int" to both
    And we send Describe "S" "s1_q1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "s1_q1" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Parse "s1_q2" with query "select $1::text" to both
    And we send Describe "S" "s1_q2" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "s1_q2" with params "hello" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Parse "s1_q3" with query "select $1::bigint" to both
    And we send Describe "S" "s1_q3" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "s1_q3" with params "100" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    # Disconnect and reconnect
    When we disconnect from both
    And we reconnect to both
    # Second session: same queries, new names
    And we send Parse "s2_q1" with query "select $1::int" to both
    And we send Describe "S" "s2_q1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "s2_q1" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Parse "s2_q2" with query "select $1::text" to both
    And we send Describe "S" "s2_q2" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "s2_q2" with params "hello" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Parse "s2_q3" with query "select $1::bigint" to both
    And we send Describe "S" "s2_q3" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "s2_q3" with params "100" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  # ============================================================================
  # PYTHON INTEGRATION TEST
  # ============================================================================

  @selectin-python
  Scenario: Run selectin test through pg_doorman
    When I run shell command:
      """
      cd tests/python && \
      python3 ./test_selectin_bug.py \
        --host 127.0.0.1 \
        --port ${DOORMAN_PORT} \
        --user example_user_1 \
        --password test \
        --dbname example_db \
        --iterations 100 \
        --workers 2
      """
    Then the command should succeed
    And the command output should contain "All queries passed successfully"

  @selectin-python
  Scenario: Run selectin test direct to PostgreSQL (baseline)
    When I run shell command:
      """
      cd tests/python && \
      python3 ./test_selectin_bug.py \
        --host 127.0.0.1 \
        --port ${PG_PORT} \
        --user example_user_1 \
        --password test \
        --dbname example_db \
        --iterations 100 \
        --workers 2
      """
    Then the command should succeed
    And the command output should contain "All queries passed successfully"
