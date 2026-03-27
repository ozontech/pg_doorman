@dotnet @pipeline-cancel-disconnect-bug
Feature: Pipeline cancel disconnect bug (.NET Npgsql reproduction)
  Exact reproduction of the bug: Client A sends a parameterized query with ~4MB text
  via Npgsql extended protocol, reads the result, then kills the TCP socket with RST.
  Client B reuses the same server connection (pool_size=1) and gets protocol violation.

  Key config: message_size_to_be_stream = 2048 (2KB) — forces DataRow streaming through
  handle_large_data_row path where error handling differs from normal buffered path.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      connect_timeout = 5000
      admin_username = "admin"
      admin_password = "admin"
      prepared_statements = true
      prepared_statements_cache_size = 10000
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      worker_threads = 1
      message_size_to_be_stream = 2048
      cleanup_server_connections = false

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  @pipeline-cancel-disconnect-bug
  Scenario: Npgsql client kills socket during 4MB streaming transfer - next client must work
    When I run shell command:
      """
      export DATABASE_URL="Host=127.0.0.1;Port=${DOORMAN_PORT};Database=example_db;Username=example_user_1;Password="
      tests/dotnet/run_test.sh pipeline_cancel_disconnect pipeline_cancel_disconnect.cs
      """
    Then the command should succeed
    And the command output should contain "Client A: Exception caught"
    And the command output should contain "Client B: Query completed successfully"
    And the command output should not contain "Bug detected"
    And the command output should contain "pipeline_cancel_disconnect complete"
