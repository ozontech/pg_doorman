@rust @rust-2 @backend-batch
Feature: Backend batch transfer while PostgreSQL returns results
  A small backend Unix socket buffer forces the request to be sent in portions.
  The client reads results immediately, so client-side backpressure is excluded.
  A slow but progressing batch must not be aborted by a fixed send deadline.

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

  @backend-duplex-large-batch
  Scenario: Large replies are drained while the request tail is still being written
    When we create session "client" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we exchange the following extended batch on session "client":
      | queries       | 2       |
      | delay_ms      | 0       |
      | row_bytes     | 2097152 |
      | padding_bytes | 2097152 |
      | timeout_ms    | 20000   |
    Then session "client" should complete all 2 batch queries without error
    And session "client" should accept another query after the batch

  @backend-batch-direct-control
  Scenario: The paced batch completes directly on PostgreSQL
    When we create session "direct" to postgres as "example_user_1" with password "" and database "example_db"
    And we exchange the following extended batch on session "direct":
      | queries       | 64    |
      | delay_ms      | 150   |
      | row_bytes     | 1024  |
      | padding_bytes | 2048  |
      | timeout_ms    | 20000 |
    Then session "direct" should receive batch rows over at least 500ms
    And session "direct" should complete all 64 batch queries without error
    And session "direct" should accept another query after the batch

  @backend-batch-progress
  Scenario: A batch taking longer than five seconds completes while replies keep arriving
    When we create session "client" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we exchange the following extended batch on session "client":
      | queries       | 64    |
      | delay_ms      | 150   |
      | row_bytes     | 1024  |
      | padding_bytes | 2048  |
      | timeout_ms    | 20000 |
    # This assertion must pass even before the deadline bug is fixed: some
    # results have actually arrived at different times before the disconnect.
    Then session "client" should receive batch rows over at least 500ms
    And session "client" should complete all 64 batch queries without error
    And session "client" should accept another query after the batch
