@dotnet @dotnet-pipeline-disconnect
Feature: .NET pipeline disconnect test
  Test that pg_doorman correctly handles client disconnect during pipeline/batch operations.
  Client A starts a batch query with large result and crashes.
  Client B should get a clean connection (same server connection from pool with pool_size=1).

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

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  Scenario: Client A crashes during batch, client B gets clean connection
    When I run shell command:
      """
      export DATABASE_URL="Host=127.0.0.1;Port=${DOORMAN_PORT};Database=example_db;Username=example_user_1;Password="
      tests/dotnet/run_test.sh pipeline_disconnect pipeline_disconnect.cs
      """
    Then the command should succeed
    And the command output should contain "Client A: Exception caught"
    And the command output should contain "Client B: All results correct!"
    And the command output should contain "pipeline_disconnect complete"
