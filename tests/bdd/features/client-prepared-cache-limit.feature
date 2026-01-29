@rust @rust-3 @client-cache-limit
Feature: Client prepared statements cache size limit (LRU eviction)
  Test that client_prepared_statements_cache_size parameter limits per-client cache
  and evicts old entries when limit is reached. This protects against malicious clients
  that don't call DEALLOCATE and could exhaust server memory.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @debug-lru
  Scenario: Client cache evicts oldest entries when limit is reached
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100
      client_prepared_statements_cache_size = 3

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Create 3 prepared statements (filling the cache)
    And we send Parse "stmt_1" with query "select 1" to session "one"
    And we send Sync to session "one"
    And we send Parse "stmt_2" with query "select 2" to session "one"
    And we send Sync to session "one"
    And we send Parse "stmt_3" with query "select 3" to session "one"
    And we send Sync to session "one"
    # Verify all 3 work
    And we send Bind "" to "stmt_1" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "1"
    When we send Bind "" to "stmt_2" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "2"
    When we send Bind "" to "stmt_3" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "3"
    # Create 4th statement - should evict stmt_1 from client cache
    When we send Parse "stmt_4" with query "select 4" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "stmt_4" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "4"
    # stmt_2, stmt_3, stmt_4 should still work
    When we send Bind "" to "stmt_2" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "2"
    When we send Bind "" to "stmt_3" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "3"

  Scenario: Default unlimited cache (client_prepared_statements_cache_size = 0)
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100
      client_prepared_statements_cache_size = 0

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Create many prepared statements - all should be cached (no eviction)
    And we send Parse "s1" with query "select 1" to session "one"
    And we send Sync to session "one"
    And we send Parse "s2" with query "select 2" to session "one"
    And we send Sync to session "one"
    And we send Parse "s3" with query "select 3" to session "one"
    And we send Sync to session "one"
    And we send Parse "s4" with query "select 4" to session "one"
    And we send Sync to session "one"
    And we send Parse "s5" with query "select 5" to session "one"
    And we send Sync to session "one"
    # All should still work
    And we send Bind "" to "s1" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "1"
    When we send Bind "" to "s5" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "5"

  Scenario: Different clients have independent cache limits
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100
      client_prepared_statements_cache_size = 2

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    When we create session "client_a" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "client_b" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Client A creates 2 statements
    And we send Parse "stmt_a1" with query "select 'a1'" to session "client_a"
    And we send Sync to session "client_a"
    And we send Parse "stmt_a2" with query "select 'a2'" to session "client_a"
    And we send Sync to session "client_a"
    # Client B creates 2 statements
    And we send Parse "stmt_b1" with query "select 'b1'" to session "client_b"
    And we send Sync to session "client_b"
    And we send Parse "stmt_b2" with query "select 'b2'" to session "client_b"
    And we send Sync to session "client_b"
    # Both clients' statements should work (each has own cache)
    And we send Bind "" to "stmt_a1" with params "" to session "client_a"
    And we send Execute "" to session "client_a"
    And we send Sync to session "client_a"
    Then session "client_a" should receive DataRow with "a1"
    When we send Bind "" to "stmt_b1" with params "" to session "client_b"
    And we send Execute "" to session "client_b"
    And we send Sync to session "client_b"
    Then session "client_b" should receive DataRow with "b1"
