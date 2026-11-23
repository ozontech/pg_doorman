@rust @rust-1 @flush-timeout
Feature: A stalled backend write ends with a proper ErrorResponse
  When server_write_idle_timeout fires (the write to PostgreSQL made no
  progress for the whole timeout, five seconds by default), the client must
  receive a PostgreSQL ErrorResponse message, not a bare TCP connection
  close. Without it, drivers like Npgsql report "protocol violation" because
  they expect PostgreSQL protocol messages but get unexpected EOF.

  The timeout is counted in pg_doorman_server_write_idle_timeouts_total with
  the side that stopped reading: cause="backend" when PostgreSQL does not
  take the request, cause="client" when the client does not take the replies.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @flush-timeout-basic
  Scenario: Client receives ErrorResponse when an extended batch stalls on a frozen backend
    Given pg_doorman started with config:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

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
    # 5. Wait for the default 5s write idle timeout to fire (plus margin)
    And we sleep 10000ms
    # 6. Try to read response - should get ErrorResponse, not TCP close
    Then session "client1" should receive ErrorResponse or connection close with error
    And session "client1" should receive ErrorResponse with SQLSTATE "58006"
    # 7. The stall is attributed to the backend
    When I run shell command:
      """
      set -e
      BACKEND=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_server_write_idle_timeouts_total\{cause="backend"\} / {print $2; f=1} END {if (!f) print 0}')
      CLIENT=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_server_write_idle_timeouts_total\{cause="client"\} / {print $2; f=1} END {if (!f) print 0}')
      echo "backend=$BACKEND client=$CLIENT"
      test "$BACKEND" = "1" || { echo "expected cause=backend 1, got $BACKEND"; exit 1; }
      test "$CLIENT" = "0" || { echo "expected cause=client 0, got $CLIENT"; exit 1; }
      """
    # 8. Unfreeze PostgreSQL for cleanup
    When we unfreeze PostgreSQL with SIGCONT

  @flush-timeout-simple-query
  Scenario: Client receives ErrorResponse when a simple query stalls on a frozen backend
    Given pg_doorman started with config:
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
    #    8MB query is enough to overflow TCP buffers when PG is frozen.
    And we send large SimpleQuery with 8192KB padding to session "client1" without waiting
    # 4. Wait for the default 5s write idle timeout to fire (plus margin)
    And we sleep 10000ms
    # 5. Try to read response - should get ErrorResponse, not TCP close
    Then session "client1" should receive ErrorResponse or connection close with error
    And session "client1" should receive ErrorResponse with SQLSTATE "58006"
    # 6. Unfreeze PostgreSQL for cleanup
    When we unfreeze PostgreSQL with SIGCONT

  @flush-timeout-client
  Scenario: A client that stops reading replies is reported as the cause
    # Small backend socket buffers make the request tail wait for PostgreSQL.
    # The client sends the whole batch and never reads: the 64 MB reply to
    # the first query fills the client connection, pg_doorman stops reading
    # it from PostgreSQL, and PostgreSQL stops reading the second query.
    Given pg_doorman started with config:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = false
      unix_socket_buffer_size = 4096
      worker_threads = 2

      [pools.example_db]
      server_host = "${PG_TEMP_DIR}"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "client1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send the following extended batch on session "client1" without reading replies:
      | queries       | 2        |
      | delay_ms      | 0        |
      | row_bytes     | 67108864 |
      | padding_bytes | 4194304  |
      | timeout_ms    | 20000    |
    # Default 5s write idle timeout, plus margin
    And we sleep 10000ms
    Then session "client1" should be closed by pg_doorman within 20000ms
    When I run shell command:
      """
      set -e
      BACKEND=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_server_write_idle_timeouts_total\{cause="backend"\} / {print $2; f=1} END {if (!f) print 0}')
      CLIENT=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_server_write_idle_timeouts_total\{cause="client"\} / {print $2; f=1} END {if (!f) print 0}')
      echo "backend=$BACKEND client=$CLIENT"
      test "$CLIENT" = "1" || { echo "expected cause=client 1, got $CLIENT"; exit 1; }
      test "$BACKEND" = "0" || { echo "expected cause=backend 0, got $BACKEND"; exit 1; }
      """
    # The pool slot is free again: another client can run a query.
    When we create session "client2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    Then session "client2" should accept another query after the batch
