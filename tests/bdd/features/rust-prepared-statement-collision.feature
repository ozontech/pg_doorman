@rust @prepared-collision
Feature: Multiple clients with same prepared statement name returning different data
  Test that multiple clients using the same prepared statement name but different queries
  receive correct responses when pool_size=1 (forcing connection sharing)

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
      pool_size = 1
      """

  Scenario: Multiple clients use same statement name with different queries - pool_size=1
    # Client 1 creates prepared statement "my_stmt" that returns 100
    When we create session "client1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "my_stmt" with query "select 100::int as result" to session "client1"
    And we send Sync to session "client1"
    And we send Bind "" to "my_stmt" with params "" to session "client1"
    And we send Execute "" to session "client1"
    And we send Sync to session "client1"
    Then session "client1" should receive DataRow with "100"

    # Client 2 creates prepared statement with SAME NAME "my_stmt" but DIFFERENT query that returns 200
    When we create session "client2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "my_stmt" with query "select 200::int as result" to session "client2"
    And we send Sync to session "client2"
    And we send Bind "" to "my_stmt" with params "" to session "client2"
    And we send Execute "" to session "client2"
    And we send Sync to session "client2"
    Then session "client2" should receive DataRow with "200"

    # Client 3 creates prepared statement with SAME NAME "my_stmt" but returns 300
    When we create session "client3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "my_stmt" with query "select 300::int as result" to session "client3"
    And we send Sync to session "client3"
    And we send Bind "" to "my_stmt" with params "" to session "client3"
    And we send Execute "" to session "client3"
    And we send Sync to session "client3"
    Then session "client3" should receive DataRow with "300"

    # Now verify each client still gets their own correct result when re-executing
    When we send Bind "" to "my_stmt" with params "" to session "client1"
    And we send Execute "" to session "client1"
    And we send Sync to session "client1"
    Then session "client1" should receive DataRow with "100"

    When we send Bind "" to "my_stmt" with params "" to session "client2"
    And we send Execute "" to session "client2"
    And we send Sync to session "client2"
    Then session "client2" should receive DataRow with "200"

    When we send Bind "" to "my_stmt" with params "" to session "client3"
    And we send Execute "" to session "client3"
    And we send Sync to session "client3"
    Then session "client3" should receive DataRow with "300"

  Scenario: Interleaved execution with same statement name - pool_size=1
    # Create all three clients first
    When we create session "alpha" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "beta" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "gamma" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Each client parses "shared_name" with different query
    And we send Parse "shared_name" with query "select 'ALPHA'::text as client" to session "alpha"
    And we send Sync to session "alpha"
    And we send Parse "shared_name" with query "select 'BETA'::text as client" to session "beta"
    And we send Sync to session "beta"
    And we send Parse "shared_name" with query "select 'GAMMA'::text as client" to session "gamma"
    And we send Sync to session "gamma"

    # Interleaved execution - execute in different order than parse
    When we send Bind "" to "shared_name" with params "" to session "gamma"
    And we send Execute "" to session "gamma"
    And we send Sync to session "gamma"
    Then session "gamma" should receive DataRow with "GAMMA"

    When we send Bind "" to "shared_name" with params "" to session "alpha"
    And we send Execute "" to session "alpha"
    And we send Sync to session "alpha"
    Then session "alpha" should receive DataRow with "ALPHA"

    When we send Bind "" to "shared_name" with params "" to session "beta"
    And we send Execute "" to session "beta"
    And we send Sync to session "beta"
    Then session "beta" should receive DataRow with "BETA"

  Scenario: Same statement name with parameters returning different computed values - pool_size=1
    When we create session "sess1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "sess2" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # sess1: my_calc multiplies by 10
    And we send Parse "my_calc" with query "select $1::int * 10 as result" to session "sess1"
    And we send Sync to session "sess1"

    # sess2: my_calc multiplies by 100
    And we send Parse "my_calc" with query "select $1::int * 100 as result" to session "sess2"
    And we send Sync to session "sess2"

    # Execute with same parameter value, expect different results
    When we send Bind "" to "my_calc" with params "5" to session "sess1"
    And we send Execute "" to session "sess1"
    And we send Sync to session "sess1"
    Then session "sess1" should receive DataRow with "50"

    When we send Bind "" to "my_calc" with params "5" to session "sess2"
    And we send Execute "" to session "sess2"
    And we send Sync to session "sess2"
    Then session "sess2" should receive DataRow with "500"

    # Execute again with different parameter
    When we send Bind "" to "my_calc" with params "7" to session "sess1"
    And we send Execute "" to session "sess1"
    And we send Sync to session "sess1"
    Then session "sess1" should receive DataRow with "70"

    When we send Bind "" to "my_calc" with params "7" to session "sess2"
    And we send Execute "" to session "sess2"
    And we send Sync to session "sess2"
    Then session "sess2" should receive DataRow with "700"

  Scenario: Rapid alternating execution between two clients - pool_size=1
    When we create session "odd" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "even" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Both use "counter" statement name but return different values
    And we send Parse "counter" with query "select 1::int as val" to session "odd"
    And we send Sync to session "odd"
    And we send Parse "counter" with query "select 2::int as val" to session "even"
    And we send Sync to session "even"

    # Rapid alternating execution
    When we send Bind "" to "counter" with params "" to session "odd"
    And we send Execute "" to session "odd"
    And we send Sync to session "odd"
    Then session "odd" should receive DataRow with "1"

    When we send Bind "" to "counter" with params "" to session "even"
    And we send Execute "" to session "even"
    And we send Sync to session "even"
    Then session "even" should receive DataRow with "2"

    When we send Bind "" to "counter" with params "" to session "odd"
    And we send Execute "" to session "odd"
    And we send Sync to session "odd"
    Then session "odd" should receive DataRow with "1"

    When we send Bind "" to "counter" with params "" to session "even"
    And we send Execute "" to session "even"
    And we send Sync to session "even"
    Then session "even" should receive DataRow with "2"

    When we send Bind "" to "counter" with params "" to session "odd"
    And we send Execute "" to session "odd"
    And we send Sync to session "odd"
    Then session "odd" should receive DataRow with "1"

    When we send Bind "" to "counter" with params "" to session "even"
    And we send Execute "" to session "even"
    And we send Sync to session "even"
    Then session "even" should receive DataRow with "2"



  Scenario: Client disconnects without Sync - next client should not receive garbage - pool_size=1
    # Client 1 sends Parse/Bind/Execute but does NOT send Sync, then disconnects
    # This tests that pg_doorman properly cleans up incomplete protocol state
    # and the next client doesn't receive leftover data
    When we create session "bad_client" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "orphan_stmt" with query "select 'GARBAGE_DATA'::text as leak" to session "bad_client"
    And we send Bind "" to "orphan_stmt" with params "" to session "bad_client"
    And we send Execute "" to session "bad_client"
    # NO Sync sent - client disconnects abruptly
    When we close session "bad_client"

    # Client 2 connects and should get clean state, not garbage from previous client
    When we create session "good_client" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "my_stmt" with query "select 'CLEAN_DATA'::text as result" to session "good_client"
    And we send Sync to session "good_client"
    And we send Bind "" to "my_stmt" with params "" to session "good_client"
    And we send Execute "" to session "good_client"
    And we send Sync to session "good_client"
    Then session "good_client" should receive DataRow with "CLEAN_DATA"

  Scenario: Multiple clients disconnect without Sync - subsequent client works correctly - pool_size=1
    # Multiple bad clients disconnect without completing protocol
    When we create session "bad1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "select 'BAD1'::text" to session "bad1"
    And we send Bind "" to "stmt" with params "" to session "bad1"
    And we send Execute "" to session "bad1"
    When we close session "bad1"

    When we create session "bad2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "select 'BAD2'::text" to session "bad2"
    And we send Bind "" to "stmt" with params "" to session "bad2"
    When we close session "bad2"

    When we create session "bad3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "select 'BAD3'::text" to session "bad3"
    When we close session "bad3"

    # Good client should work correctly
    When we create session "good" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "clean_stmt" with query "select 999::int as val" to session "good"
    And we send Sync to session "good"
    And we send Bind "" to "clean_stmt" with params "" to session "good"
    And we send Execute "" to session "good"
    And we send Sync to session "good"
    Then session "good" should receive DataRow with "999"

  Scenario: Interleaved good and bad clients - pool_size=1
    # Good client 1 works normally
    When we create session "good1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "select 'GOOD1'::text as val" to session "good1"
    And we send Sync to session "good1"
    And we send Bind "" to "stmt" with params "" to session "good1"
    And we send Execute "" to session "good1"
    And we send Sync to session "good1"
    Then session "good1" should receive DataRow with "GOOD1"

    # Bad client disconnects without Sync
    When we create session "bad" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "select 'LEAKED'::text as val" to session "bad"
    And we send Bind "" to "stmt" with params "" to session "bad"
    And we send Execute "" to session "bad"
    When we close session "bad"

    # Good client 2 should work correctly and not see LEAKED data
    When we create session "good2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt" with query "select 'GOOD2'::text as val" to session "good2"
    And we send Sync to session "good2"
    And we send Bind "" to "stmt" with params "" to session "good2"
    And we send Execute "" to session "good2"
    And we send Sync to session "good2"
    Then session "good2" should receive DataRow with "GOOD2"

    # Good client 1 should still work
    When we send Bind "" to "stmt" with params "" to session "good1"
    And we send Execute "" to session "good1"
    And we send Sync to session "good1"
    Then session "good1" should receive DataRow with "GOOD1"
