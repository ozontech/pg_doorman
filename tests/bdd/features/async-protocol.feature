@rust @rust-1 @async-protocol
Feature: Async Protocol (Extended Query with Flush)
  Testing async protocol support - sending Parse/Bind/Describe/Execute with Flush instead of Sync
  to ensure pg_doorman handles async operations identically to PostgreSQL

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
      pool_size = 10
      """
  @async-protocol-first
  Scenario: Parse + Flush gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "" with params "1" to both
    And we send Describe "P" "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-second
  Scenario: Parse + Bind + Flush gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int" to both
    And we send Bind "" to "" with params "1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Describe "P" "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-third
  Scenario: Parse + Bind + Describe + Flush gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int" to both
    And we send Bind "" to "" with params "1" to both
    And we send Describe "P" "" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-fourth
  Scenario: Multiple Parse + Flush operations
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Parse "" with query "select $1::int, $2::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "" with params "1, 2" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-fifth
  Scenario: Named prepared statement with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "stmt1" with params "42" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-sixth
  Scenario: Reuse named prepared statement with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "stmt1" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Bind "" to "stmt1" with params "2" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-seventh
  Scenario: Multiple named statements with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "stmt1" with params "42" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Bind "" to "stmt2" with params "hello" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-eighth
  Scenario: Parse error with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "bad sql syntax" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-ninth
  Scenario: Parse + Bind + Execute without Flush (baseline)
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int" to both
    And we send Bind "" to "" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-tenth
  Scenario: Complex query with Flush after each step
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int + $2::int as sum" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "" with params "10, 20" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Describe "P" "" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-eleventh
  Scenario: Portal operations with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int" to both
    And we send Bind "portal1" to "" with params "1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Describe "P" "portal1" to both
    And we send Execute "portal1" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-twelfth
  Scenario: Close statement with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Close "S" "stmt1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-thirteenth
  Scenario: Close portal with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int" to both
    And we send Bind "portal1" to "" with params "1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Close "P" "portal1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-fourteenth
  Scenario: Partial execution with max_rows and Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select generate_series(1, 10)" to both
    And we send Bind "" to "" with params "" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Execute "" with max_rows "3" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Execute "" with max_rows "3" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Execute "" with max_rows "0" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-fifteen
  Scenario: Interleaved Parse and Bind with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "stmt1" with params "1" to both
    And we send Bind "" to "stmt2" with params "test" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-sixteen
  Scenario: Prepared statement cache with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "cached_stmt" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "cached_stmt" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Bind "" to "cached_stmt" with params "2" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Bind "" to "cached_stmt" with params "3" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-seventeen
  Scenario: Transaction with Parse + Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to both
    And we send Parse "" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send SimpleQuery "COMMIT" to both
    Then we should receive identical messages from both

  @async-protocol-eighteen
  Scenario: Transaction rollback with Parse + Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to both
    And we send Parse "" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send SimpleQuery "ROLLBACK" to both
    Then we should receive identical messages from both

  @async-protocol-nineteen
  Scenario: Multiple statements in transaction with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to both
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "stmt1" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Bind "" to "stmt2" with params "test" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send SimpleQuery "COMMIT" to both
    Then we should receive identical messages from both

  @async-protocol-twenty
  Scenario: Error in transaction with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to both
    And we send Parse "" with query "invalid sql" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send SimpleQuery "ROLLBACK" to both
    Then we should receive identical messages from both

  @async-protocol-twenty-one
  Scenario: Prepared statement with multiple parameter types and Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int, $2::text, $3::bool" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "" with params "42, hello, true" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-twenty-two
  Scenario: Reparse same anonymous statement with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
    When we send Parse "" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "" with params "2" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  Scenario: Large number of Parse operations with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select 1" to both
    And we send Parse "stmt2" with query "select 2" to both
    And we send Parse "stmt3" with query "select 3" to both
    And we send Parse "stmt4" with query "select 4" to both
    And we send Parse "stmt5" with query "select 5" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "stmt3" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-twenty-three
  Scenario: Describe statement before Bind with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Describe "S" "stmt1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "stmt1" with params "1" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-twenty-four
  Scenario: Parse + Describe + Flush for anonymous statement (asyncpg pattern)
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT 1 as col" to both
    And we send Describe "S" "" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @async-protocol-twenty-five
  Scenario: Parse + Describe + Flush for named statement (asyncpg COPY pattern)
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "__asyncpg_stmt_1__" with query "SELECT 1 as col" to both
    And we send Describe "S" "__asyncpg_stmt_1__" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "__asyncpg_stmt_1__" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  Scenario: Empty query with Flush
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select 1" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
