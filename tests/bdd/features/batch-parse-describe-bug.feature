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
    # First, prepare stmt1 - this will be cached
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Describe "S" "stmt1" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-bug-step2
  Scenario: Step 2 - Reproduce the bug with batch containing cached Parse + new Parse + Describe
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    # First, prepare stmt1 - this will be cached on server
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    # Now send a batch that triggers the bug:
    # - Parse stmt1 again (will be skipped, already cached)
    # - Parse stmt2 (new, will be sent to server)
    # - Describe stmt1 (for the cached/skipped statement)
    #
    # Expected from PostgreSQL: 1 ParseComplete (for stmt2) + ParameterDescription + RowDescription
    # Bug: pg_doorman sends 2 ParseComplete (one injected for skipped stmt1)
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Describe "S" "stmt1" to both
    And we send Sync to both
    Then we should receive identical messages from both

  @batch-bug-step3
  Scenario: Step 3 - More complex batch with multiple cached and new statements
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    # Prepare stmt1 first
    And we send Parse "stmt1" with query "select $1::int" to both
    And we send Sync to both
    # Prepare stmt2
    And we send Parse "stmt2" with query "select $1::text" to both
    And we send Sync to both
    # Now batch with:
    # - Parse stmt2 (cached/skipped)
    # - Parse stmt3 (new)
    # - Describe stmt1
    # - Describe stmt2
    And we send Parse "stmt3" with query "select $1::bigint" to both
    And we send Describe "S" "stmt1" to both
    And we send Describe "S" "stmt2" to both
    And we send Sync to both
    Then we should receive identical messages from both