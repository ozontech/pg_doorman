@rust @rust-2 @pipeline
Feature: Asynchronous Pipeline Protocol
  Test pg_doorman pipeline mode (asynchronous extended protocol)
  
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

  @pipeline-first
  Scenario: Pipeline multiple queries without intermediate Sync
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Bind "portal1" to "stmt1" with params "1" to both
    And we send Execute "portal1" to both
    And we send Parse "stmt2" with query "select $1::int, $2::int" to both
    And we send Bind "portal2" to "stmt2" with params "2, 3" to both
    And we send Execute "portal2" to both
    And we send Parse "stmt3" with query "select $1::text" to both
    And we send Bind "portal3" to "stmt3" with params "hello" to both
    And we send Execute "portal3" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-second
  Scenario: Pipeline with partial result consumption using max_rows
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT * FROM generate_series(1, 100)" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" with max_rows "10" to both
    And we send Parse "" with query "select 42" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-third
  Scenario: Interleaved portal operations
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Bind "portal1" to "stmt1" with params "100" to both
    And we send Bind "portal2" to "stmt2" with params "test" to both
    And we send Execute "portal1" to both
    And we send Execute "portal2" to both
    And we send Close "P" "portal1" to both
    And we send Close "S" "stmt1" to both
    And we send Execute "portal2" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-fourth
  Scenario: Multiple named statements without immediate execution
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "query1" with query "select 1" to both
    And we send Parse "query2" with query "select 2" to both
    And we send Parse "query3" with query "select 3" to both
    And we send Describe "S" "query1" to both
    And we send Describe "S" "query2" to both
    And we send Describe "S" "query3" to both
    And we send Bind "" to "query2" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-fifth
  Scenario: Error in middle of pipeline should skip subsequent commands until Sync
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select 1" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Parse "" with query "invalid sql syntax" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Parse "" with query "select 2" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-sixth
  Scenario: Multiple errors in pipeline
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "bad query 1" to both
    And we send Parse "" with query "bad query 2" to both
    And we send Parse "" with query "select 1" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-seventh
  Scenario: Flush forces immediate response without Sync
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select 1" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Parse "" with query "select 2" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-eighth
  Scenario: Multiple Flush commands in pipeline
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select 1" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Parse "" with query "select 2" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Flush to both
    And we verify partial response received from both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-ninth
  Scenario: Async cursor with partial fetches
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT * FROM generate_series(1, 100)" to both
    And we send Bind "cursor1" to "" with params "" to both
    And we send Execute "cursor1" with max_rows "10" to both
    And we send Execute "cursor1" with max_rows "10" to both
    And we send Execute "cursor1" with max_rows "10" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-tenth
  Scenario: Multiple cursors in pipeline
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "SELECT * FROM generate_series(1, 50)" to both
    And we send Parse "stmt2" with query "SELECT * FROM generate_series(100, 150)" to both
    And we send Bind "cursor1" to "stmt1" with params "" to both
    And we send Bind "cursor2" to "stmt2" with params "" to both
    And we send Execute "cursor1" with max_rows "5" to both
    And we send Execute "cursor2" with max_rows "5" to both
    And we send Execute "cursor1" with max_rows "5" to both
    And we send Execute "cursor2" with max_rows "5" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-eleventh
  Scenario: Pipeline with 500 queries without Sync
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we repeat 500 times: Parse "" with query "select $1::int", Bind "" to "" with params "1", Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-twelfth
  Scenario: Mixed pipeline operations at scale
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we repeat 100 times: Parse "" with query "select $1::int", Bind "" to "" with params "1", Describe "P" "", Execute "", Close "P" "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-thirteenth
  Scenario: Multiple transactions in pipeline
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "BEGIN" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Parse "" with query "select 1" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Parse "" with query "COMMIT" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @pipeline-fourteenth
  Scenario: Rollback in pipeline after error
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "BEGIN" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Parse "" with query "invalid sql" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Parse "" with query "ROLLBACK" to both
    And we send Bind "" to "" with params "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
