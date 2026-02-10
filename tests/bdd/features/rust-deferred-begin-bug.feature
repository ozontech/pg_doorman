@rust @rust-deferred-begin-bug
Feature: Deferred BEGIN optimization bug with extended protocol
  Test that pg_doorman correctly handles transaction state when deferred BEGIN
  is followed by ROLLBACK without any actual queries in extended protocol.

  This reproduces the bug where:
  1. Client sends BEGIN (deferred - no server connection allocated)
  2. pg_doorman responds with ReadyForQuery('T') synthetically
  3. Client sends ROLLBACK and closes connection
  4. Connection returned to pool with stale transaction state
  5. Next client reuses connection and gets InterfaceError

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

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  @deferred-begin-extended-empty-transaction
  Scenario: Empty transaction with extended protocol causes state desync
    # This test reproduces the bug using pure extended protocol

    # Session 1: Create connection, send BEGIN via extended protocol
    When we create extended session "session1" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Send Parse for BEGIN
    And we send Parse "" with query "BEGIN" to session "session1"
    And we send Bind "" to "" with params "" to session "session1"
    And we send Execute "" to session "session1"
    And we send Sync to session "session1"

    # Verify we get ReadyForQuery('T') - pg_doorman sends synthetic response
    Then session "session1" should receive ParseComplete
    And session "session1" should receive BindComplete
    And session "session1" should receive CommandComplete "BEGIN"
    And session "session1" should receive ReadyForQuery "T"

    # Check that no server backend is actually allocated
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pools" on admin session "admin" and store response
    # sv_active should be 0 because BEGIN was deferred
    Then admin session "admin" column "sv_active" should be between 0 and 0

    # Now send ROLLBACK and disconnect
    And we send Parse "" with query "ROLLBACK" to session "session1"
    And we send Bind "" to "" with params "" to session "session1"
    And we send Execute "" to session "session1"
    And we send Sync to session "session1"

    Then session "session1" should receive ParseComplete
    And session "session1" should receive BindComplete
    And session "session1" should receive CommandComplete "ROLLBACK"
    And session "session1" should receive ReadyForQuery "I"

    # Disconnect session1
    And we disconnect session "session1"

    # Session 2: Reuse the connection from pool
    # BUG: If pg_doorman doesn't properly reset transaction state,
    # the next client might see stale state
    When we create extended session "session2" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Try to start a new transaction
    And we send Parse "" with query "BEGIN" to session "session2"
    And we send Bind "" to "" with params "" to session "session2"
    And we send Execute "" to session "session2"
    And we send Sync to session "session2"

    # Should succeed without errors
    Then session "session2" should receive ParseComplete
    And session "session2" should receive BindComplete
    And session "session2" should receive CommandComplete "BEGIN"
    And session "session2" should receive ReadyForQuery "T"

    # Execute a real query to force server allocation
    And we send Parse "" with query "SELECT 1" to session "session2"
    And we send Bind "" to "" with params "" to session "session2"
    And we send Execute "" to session "session2"
    And we send Sync to session "session2"

    Then session "session2" should receive ParseComplete
    And session "session2" should receive BindComplete
    And session "session2" should receive RowDescription with 1 fields
    And session "session2" should receive DataRow
    And session "session2" should receive CommandComplete "SELECT 1"
    And session "session2" should receive ReadyForQuery "T"

  @deferred-begin-simple-protocol-empty-transaction
  Scenario: Empty transaction with simple protocol causes state desync
    # Same test but using simple query protocol

    When we create session "client1" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Send BEGIN - deferred optimization
    And we send SimpleQuery "BEGIN" to session "client1"

    # Verify ReadyForQuery('T') received
    Then session "client1" should receive CommandComplete "BEGIN"
    And session "client1" should receive ReadyForQuery "T"

    # Check no server backend allocated
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pools" on admin session "admin" and store response
    Then admin session "admin" column "sv_active" should be between 0 and 0

    # Send ROLLBACK and close
    And we send SimpleQuery "ROLLBACK" to session "client1"
    Then session "client1" should receive CommandComplete "ROLLBACK"
    And session "client1" should receive ReadyForQuery "I"

    And we disconnect session "client1"

    # New session should work fine
    When we create session "client2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 42" to session "client2"

    Then session "client2" should receive RowDescription with 1 fields
    And session "client2" should receive DataRow
    And session "client2" should receive CommandComplete "SELECT 1"
    And session "client2" should receive ReadyForQuery "I"

  @deferred-begin-multiple-empty-transactions
  Scenario: Multiple empty transactions in sequence
    # Stress test: multiple BEGIN/ROLLBACK cycles without queries

    When we create extended session "client" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Iteration 1: BEGIN + ROLLBACK
    And we send Parse "" with query "BEGIN" to session "client"
    And we send Bind "" to "" with params "" to session "client"
    And we send Execute "" to session "client"
    And we send Sync to session "client"

    Then session "client" should receive ParseComplete
    And session "client" should receive BindComplete
    And session "client" should receive CommandComplete "BEGIN"
    And session "client" should receive ReadyForQuery "T"

    And we send Parse "" with query "ROLLBACK" to session "client"
    And we send Bind "" to "" with params "" to session "client"
    And we send Execute "" to session "client"
    And we send Sync to session "client"

    Then session "client" should receive ParseComplete
    And session "client" should receive BindComplete
    And session "client" should receive CommandComplete "ROLLBACK"
    And session "client" should receive ReadyForQuery "I"

    # Iteration 2: BEGIN + ROLLBACK again
    And we send Parse "" with query "BEGIN" to session "client"
    And we send Bind "" to "" with params "" to session "client"
    And we send Execute "" to session "client"
    And we send Sync to session "client"

    Then session "client" should receive ParseComplete
    And session "client" should receive BindComplete
    And session "client" should receive CommandComplete "BEGIN"
    And session "client" should receive ReadyForQuery "T"

    And we send Parse "" with query "ROLLBACK" to session "client"
    And we send Bind "" to "" with params "" to session "client"
    And we send Execute "" to session "client"
    And we send Sync to session "client"

    Then session "client" should receive ParseComplete
    And session "client" should receive BindComplete
    And session "client" should receive CommandComplete "ROLLBACK"
    And session "client" should receive ReadyForQuery "I"

    # Final check: execute a real transaction with query
    And we send Parse "" with query "BEGIN" to session "client"
    And we send Bind "" to "" with params "" to session "client"
    And we send Execute "" to session "client"
    And we send Sync to session "client"

    Then session "client" should receive ParseComplete
    And session "client" should receive BindComplete
    And session "client" should receive CommandComplete "BEGIN"
    And session "client" should receive ReadyForQuery "T"

    And we send Parse "" with query "SELECT 999" to session "client"
    And we send Bind "" to "" with params "" to session "client"
    And we send Execute "" to session "client"
    And we send Sync to session "client"

    Then session "client" should receive ParseComplete
    And session "client" should receive BindComplete
    And session "client" should receive RowDescription with 1 fields
    And session "client" should receive DataRow
    And session "client" should receive CommandComplete "SELECT 1"
    And session "client" should receive ReadyForQuery "T"

  @deferred-begin-rollback-in-pipeline
  Scenario: Deferred BEGIN followed by ROLLBACK in pipeline mode
    # Test async pipeline mode with deferred BEGIN

    When we create extended session "client" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Send BEGIN + ROLLBACK in pipeline (multiple messages before Sync)
    And we send Parse "" with query "BEGIN" to session "client"
    And we send Bind "" to "" with params "" to session "client"
    And we send Execute "" to session "client"
    And we send Parse "" with query "ROLLBACK" to session "client"
    And we send Bind "" to "" with params "" to session "client"
    And we send Execute "" to session "client"
    And we send Sync to session "client"

    # Should receive all responses
    Then session "client" should receive ParseComplete
    And session "client" should receive BindComplete
    And session "client" should receive CommandComplete "BEGIN"
    And session "client" should receive ParseComplete
    And session "client" should receive BindComplete
    And session "client" should receive CommandComplete "ROLLBACK"
    And session "client" should receive ReadyForQuery "I"

    # Verify we can execute query after pipeline BEGIN/ROLLBACK
    And we send Parse "" with query "SELECT 777" to session "client"
    And we send Bind "" to "" with params "" to session "client"
    And we send Execute "" to session "client"
    And we send Sync to session "client"

    Then session "client" should receive ParseComplete
    And session "client" should receive BindComplete
    And session "client" should receive RowDescription with 1 fields
    And session "client" should receive DataRow
    And session "client" should receive CommandComplete "SELECT 1"
    And session "client" should receive ReadyForQuery "I"
