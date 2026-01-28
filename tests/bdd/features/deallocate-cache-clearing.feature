@rust @rust-3 @deallocate-cache
Feature: DEALLOCATE clears client prepared statements cache
  Test that DEALLOCATE and DEALLOCATE ALL properly clear client-side prepared statements cache
  to prevent memory exhaustion from long-running connections

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
      prepared_statements_cache_size = 100

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """

  Scenario: DEALLOCATE ALL clears client cache and allows re-creation of statements
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Create several prepared statements
    And we send Parse "stmt_a" with query "select $1::int + 1" to session "one"
    And we send Sync to session "one"
    And we send Parse "stmt_b" with query "select $1::int + 2" to session "one"
    And we send Sync to session "one"
    And we send Parse "stmt_c" with query "select $1::int + 3" to session "one"
    And we send Sync to session "one"
    # Execute stmt_a to verify it works
    And we send Bind "" to "stmt_a" with params "10" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "11"
    # Deallocate all
    When we send SimpleQuery "DEALLOCATE ALL" to session "one"
    # Re-create stmt_a with DIFFERENT query (same name but different logic)
    And we send Parse "stmt_a" with query "select $1::int * 100" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "stmt_a" with params "10" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    # Should get result from NEW query (1000), not old query (11)
    Then session "one" should receive DataRow with "1000"

  Scenario: DEALLOCATE specific statement clears only that statement from cache
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Create two prepared statements
    And we send Parse "keep_stmt" with query "select $1::int + 100" to session "one"
    And we send Sync to session "one"
    And we send Parse "remove_stmt" with query "select $1::int + 200" to session "one"
    And we send Sync to session "one"
    # Verify both work
    And we send Bind "" to "keep_stmt" with params "1" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "101"
    When we send Bind "" to "remove_stmt" with params "1" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "201"
    # Deallocate only remove_stmt
    When we send SimpleQuery "DEALLOCATE remove_stmt" to session "one"
    # keep_stmt should still work
    And we send Bind "" to "keep_stmt" with params "5" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "105"
    # Re-create remove_stmt with different query
    When we send Parse "remove_stmt" with query "select $1::int * 10" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "remove_stmt" with params "5" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    # Should get result from NEW query (50), not old query (205)
    Then session "one" should receive DataRow with "50"

  Scenario: Case insensitive DEALLOCATE ALL
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "my_stmt" with query "select $1::int + 1" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "my_stmt" with params "5" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "6"
    # Use lowercase deallocate all
    When we send SimpleQuery "deallocate all" to session "one"
    # Re-create with different query
    And we send Parse "my_stmt" with query "select $1::int * 2" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "my_stmt" with params "5" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "10"
