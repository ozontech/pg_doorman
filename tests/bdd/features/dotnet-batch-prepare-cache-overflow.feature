@dotnet @batch-prepare-overflow @bug
Feature: Concurrent NpgsqlBatch overflows pg_doorman prepared statement cache
  Several npgsql clients concurrently issue NpgsqlBatch.PrepareAsync against a
  pg_doorman whose prepared_statements_cache_size is much smaller than the
  number of distinct prepared statements in flight. Without the deferred
  eviction-Close mechanism, pg_doorman closes a statement on PostgreSQL while
  a Bind referencing it is still queued, and the batch fails with:

      prepared statement "DOORMAN_N" does not exist

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      connect_timeout = 5000
      admin_username = "admin"
      admin_password = "admin"
      prepared_statements = true
      prepared_statements_cache_size = 1

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 1
      """

  Scenario: 16 concurrent clients with cache_size=1 and pool_size=1
    When I run shell command:
      """
      export DATABASE_URL="Host=127.0.0.1;Port=${DOORMAN_PORT};Database=example_db;Username=example_user_1;Password=test"
      tests/dotnet/run_test.sh batch_prepare_cache_overflow batch_prepare_cache_overflow.cs
      """
    Then the command should succeed
    And the command output should contain "batch_prepare_cache_overflow complete"
