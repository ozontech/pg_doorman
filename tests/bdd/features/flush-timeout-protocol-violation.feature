@flush-timeout
Feature: Flush timeout should send proper ErrorResponse to client
  When pg_doorman's 5s flush timeout fires (server TCP write blocks),
  the client must receive a PostgreSQL ErrorResponse message, not a
  bare TCP connection close. Without proper error handling, drivers
  like Npgsql report "protocol violation" because they expect
  PostgreSQL protocol messages but get unexpected EOF.

  Bug: FlushTimeout error propagates via ? through handle_sync_flush →
  transaction loop → handle(), exiting without sending ErrorResponse
  to the client. The TCP connection is simply dropped.

  @flush-timeout-basic
  Scenario: Client receives ErrorResponse on extended query protocol flush timeout
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
      prepared_statements = false

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    # 1. Connect to pg_doorman, do a warmup query to establish server connection
    When we create session "client1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "client1"
    # 2. Freeze PostgreSQL so that TCP writes from pg_doorman will eventually block
    When we freeze PostgreSQL with SIGSTOP
    # 3. Send a large batch of Parse messages to fill TCP send buffer.
    #    With prepared_statements = false, Parse messages are buffered
    #    in pg_doorman and sent to server on Sync.
    #    With all PG processes frozen (SIGSTOP), TCP recv buffer stays
    #    small (~128KB-256KB), so ~8MB is enough to overflow.
    And we send large batch of 500 Parse messages with 16KB queries to session "client1"
    # 4. Send Sync to trigger the server roundtrip (don't wait - it will timeout)
    And we send Sync to session "client1" without waiting for response
    # 5. Wait for the 5s flush timeout to fire (plus margin for reading buffered Parse messages)
    And we sleep 10000ms
    # 6. Try to read response - should get ErrorResponse, not TCP close
    Then session "client1" should receive ErrorResponse or connection close with error
    # 7. Unfreeze PostgreSQL for cleanup
    When we unfreeze PostgreSQL with SIGCONT

  @flush-timeout-simple-query
  Scenario: Client receives ErrorResponse on simple query flush timeout
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

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    # 1. Connect to pg_doorman, do a warmup query to establish server connection
    When we create session "client1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "client1"
    # 2. Freeze PostgreSQL so that TCP writes from pg_doorman will eventually block
    When we freeze PostgreSQL with SIGSTOP
    # 3. Send a large SimpleQuery to fill TCP send buffer.
    #    pg_doorman forwards SimpleQuery directly to the server via
    #    handle_simple_query → execute_server_roundtrip → send_and_flush_timeout.
    #    8MB query is enough to overflow TCP buffers when PG is frozen.
    And we send large SimpleQuery with 8192KB padding to session "client1" without waiting
    # 4. Wait for the 5s flush timeout to fire (plus margin)
    And we sleep 10000ms
    # 5. Try to read response - should get ErrorResponse, not TCP close
    Then session "client1" should receive ErrorResponse or connection close with error
    # 6. Unfreeze PostgreSQL for cleanup
    When we unfreeze PostgreSQL with SIGCONT
