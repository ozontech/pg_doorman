@rust @prepared-cache
Feature: Prepared statements caching across multiple sessions
  Test that prepared statements are properly cached and reused across different client sessions

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

  Scenario: Prepared statement is cached and reused across sessions
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int + $2::int" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "stmt1" with params "10, 20" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "30"
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int + $2::int" to session "two"
    And we send Sync to session "two"
    And we send Bind "" to "stmt1" with params "5, 15" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "20"

  Scenario: Multiple sessions use different prepared statements
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "three" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "add_stmt" with query "select $1::int + $2::int" to session "one"
    And we send Sync to session "one"
    And we send Parse "mul_stmt" with query "select $1::int * $2::int" to session "two"
    And we send Sync to session "two"
    And we send Parse "sub_stmt" with query "select $1::int - $2::int" to session "three"
    And we send Sync to session "three"
    When we send Bind "" to "add_stmt" with params "10, 5" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "15"
    When we send Bind "" to "mul_stmt" with params "10, 5" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "50"
    When we send Bind "" to "sub_stmt" with params "10, 5" to session "three"
    And we send Execute "" to session "three"
    And we send Sync to session "three"
    Then session "three" should receive DataRow with "5"

  Scenario: Prepared statement reused after session closes
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "persistent_stmt" with query "select $1::text || ' world'" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "persistent_stmt" with params "hello" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "hello world"
    When we close session "one"
    And we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "persistent_stmt" with query "select $1::text || ' world'" to session "two"
    And we send Sync to session "two"
    And we send Bind "" to "persistent_stmt" with params "goodbye" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "goodbye world"

  Scenario: Multiple sessions execute same prepared statement concurrently
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "three" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "shared_stmt" with query "select $1::int * 2" to session "one"
    And we send Sync to session "one"
    And we send Parse "shared_stmt" with query "select $1::int * 2" to session "two"
    And we send Sync to session "two"
    And we send Parse "shared_stmt" with query "select $1::int * 2" to session "three"
    And we send Sync to session "three"
    When we send Bind "" to "shared_stmt" with params "1" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "shared_stmt" with params "2" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    And we send Bind "" to "shared_stmt" with params "3" to session "three"
    And we send Execute "" to session "three"
    And we send Sync to session "three"
    Then session "one" should receive DataRow with "2"
    And session "two" should receive DataRow with "4"
    And session "three" should receive DataRow with "6"

  Scenario: Unnamed prepared statement in one session does not affect another
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int + 100" to session "one"
    And we send Bind "" to "" with params "1" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "101"
    When we send Parse "" with query "select $1::int + 200" to session "two"
    And we send Bind "" to "" with params "1" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "201"

  Scenario: Cache limit - old statements are evicted
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt_001" with query "select 1" to session "one"
    And we send Sync to session "one"
    And we send Parse "stmt_002" with query "select 2" to session "one"
    And we send Sync to session "one"
    And we send Parse "stmt_003" with query "select 3" to session "one"
    And we send Sync to session "one"
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt_002" with query "select 2" to session "two"
    And we send Sync to session "two"
    And we send Bind "" to "stmt_002" with params "" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "2"
