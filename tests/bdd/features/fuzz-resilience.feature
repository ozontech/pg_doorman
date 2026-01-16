@fuzz
Feature: Fuzz resilience - pg_doorman handles malformed messages gracefully
  Test that pg_doorman correctly handles invalid/broken/malicious messages:
  1. pg_doorman does not crash (no panic, no segfault)
  2. New clients can connect after attacks
  3. No race conditions or deadlocks
  4. Connection pool state is not corrupted

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
      pool_mode = "transaction"

      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  # ===========================================================================
  # Broken Headers Tests - fuzz first, then verify new client works
  # ===========================================================================

  @fuzz-one
  Scenario: New client works after fuzzer sends broken length header
    When fuzzer connects and sends broken length header
    # Fuzzer already connected and authenticated before sending malformed data
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'STILL_WORKS'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "STILL_WORKS"
    And pg_doorman should still be running

  Scenario: New client works after fuzzer sends negative length
    When fuzzer connects and sends negative length
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'NEGATIVE_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "NEGATIVE_OK"
    And pg_doorman should still be running

  Scenario: New client works after fuzzer sends truncated message
    When fuzzer connects and sends truncated message
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'TRUNCATED_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "TRUNCATED_OK"
    And pg_doorman should still be running

  # ===========================================================================
  # Invalid Message Types Tests
  # ===========================================================================

  Scenario: New client works after fuzzer sends unknown message type
    When fuzzer connects and sends unknown message type 'X'
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'UNKNOWN_TYPE_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "UNKNOWN_TYPE_OK"
    And pg_doorman should still be running

  Scenario: New client works after fuzzer sends server-only message type
    When fuzzer connects and sends server-only message type 'T'
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'SERVER_TYPE_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "SERVER_TYPE_OK"
    And pg_doorman should still be running

  Scenario: New client works after fuzzer sends null byte message type
    When fuzzer connects and sends null byte message type
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'NULL_TYPE_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "NULL_TYPE_OK"
    And pg_doorman should still be running

  # ===========================================================================
  # Gigantic Messages Tests
  # ===========================================================================

  Scenario: pg_doorman does not crash on gigantic message length claim
    When fuzzer connects and sends message with 256MB length claim
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'GIGANTIC_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "GIGANTIC_OK"
    And pg_doorman should still be running

  # ===========================================================================
  # Protocol Violations Tests
  # ===========================================================================

  Scenario: New client works after fuzzer sends Execute without Bind
    When fuzzer connects, authenticates, and sends Execute without Bind
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'EXECUTE_NO_BIND_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "EXECUTE_NO_BIND_OK"
    And pg_doorman should still be running

  Scenario: New client works after fuzzer sends Bind to nonexistent statement
    When fuzzer connects, authenticates, and sends Bind to nonexistent statement
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'BIND_NONEXISTENT_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "BIND_NONEXISTENT_OK"
    And pg_doorman should still be running

  # ===========================================================================
  # Random Attacks Tests
  # ===========================================================================

  Scenario: New client works after fuzzer sends random garbage
    When fuzzer sends random garbage data
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'GARBAGE_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "GARBAGE_OK"
    And pg_doorman should still be running

  @fuzz-todo
  Scenario: Connection pool stays healthy after multiple fuzzer attacks
    When fuzzer attacks with 10 random malformed connections
    And we create session "client1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "client2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q1" with query "SELECT 1" to session "client1"
    And we send Sync to session "client1"
    And we send Bind "" to "q1" with params "" to session "client1"
    And we send Execute "" to session "client1"
    And we send Sync to session "client1"
    Then session "client1" should receive DataRow with "1"
    And we send Parse "q2" with query "SELECT 2" to session "client2"
    And we send Sync to session "client2"
    And we send Bind "" to "q2" with params "" to session "client2"
    And we send Execute "" to session "client2"
    And we send Sync to session "client2"
    Then session "client2" should receive DataRow with "2"
    And pg_doorman should still be running

  # ===========================================================================
  # Combined Attacks Tests
  # ===========================================================================

  Scenario: Multiple broken headers in sequence do not crash pg_doorman
    When fuzzer connects and sends broken length header
    And fuzzer connects and sends negative length
    And fuzzer connects and sends truncated message
    And fuzzer connects and sends unknown message type 'Y'
    And fuzzer connects and sends server-only message type 'T'
    And fuzzer connects and sends null byte message type
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'MULTI_ATTACK_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "MULTI_ATTACK_OK"
    And pg_doorman should still be running

  Scenario: Stress test - many random attacks then new client
    When fuzzer attacks with 50 random malformed connections
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'STRESS_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "STRESS_OK"
    And pg_doorman should still be running

  Scenario: Large random data does not crash pg_doorman
    When fuzzer connects and sends 10000 bytes of random data
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'LARGE_RANDOM_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "LARGE_RANDOM_OK"
    And pg_doorman should still be running

  # ===========================================================================
  # Connection Abort Tests - valid data then abrupt disconnect
  # ===========================================================================

  Scenario: Client aborts after Parse - new client works
    # Client sends valid Parse then abruptly disconnects
    When we create session "abort1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "SELECT generate_series(1, 100)" to session "abort1"
    And we close session "abort1"
    # New client should work fine
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'ABORT_PARSE_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "ABORT_PARSE_OK"
    And pg_doorman should still be running

  Scenario: Client aborts after Bind - new client works
    # Client sends valid Parse+Bind then abruptly disconnects
    When we create session "abort2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "SELECT generate_series(1, 100)" to session "abort2"
    And we send Bind "" to "stmt" with params "" to session "abort2"
    And we close session "abort2"
    # New client should work fine
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'ABORT_BIND_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "ABORT_BIND_OK"
    And pg_doorman should still be running

  Scenario: Client aborts after Execute without Sync - new client works
    # Client sends valid Parse+Bind+Execute but no Sync, then disconnects
    When we create session "abort3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "SELECT generate_series(1, 100)" to session "abort3"
    And we send Bind "" to "stmt" with params "" to session "abort3"
    And we send Execute "" to session "abort3"
    And we close session "abort3"
    # New client should work fine
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'ABORT_EXECUTE_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "ABORT_EXECUTE_OK"
    And pg_doorman should still be running

  Scenario: Client sends valid query then abruptly disconnects
    # Client sends valid query, receives response, then abruptly disconnects
    When we create session "mixed" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q1" with query "SELECT 1" to session "mixed"
    And we send Sync to session "mixed"
    And we send Bind "" to "q1" with params "" to session "mixed"
    And we send Execute "" to session "mixed"
    And we send Sync to session "mixed"
    Then session "mixed" should receive DataRow with "1"
    # Now client abruptly disconnects
    When we close session "mixed"
    # New client should work fine
    And we create session "valid" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "q" with query "SELECT 'MIXED_OK'" to session "valid"
    And we send Sync to session "valid"
    And we send Bind "" to "q" with params "" to session "valid"
    And we send Execute "" to session "valid"
    And we send Sync to session "valid"
    Then session "valid" should receive DataRow with "MIXED_OK"
    And pg_doorman should still be running
