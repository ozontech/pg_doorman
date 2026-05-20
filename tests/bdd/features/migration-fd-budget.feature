Feature: pg_doorman stays within its fd budget
  Two failure modes for the same root issue — pg_doorman must keep its
  file-descriptor table inside the configured `LimitNOFILE` and reject
  clients gracefully when load exceeds `max_connections`, rather than
  exhaust the fd table and start spamming EMFILE on every new socket
  (including Patroni-fallback discovery, which then dominates the log).

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @client-migration @migration-fd-budget
  Scenario: Binary upgrade with 100 idle clients under NOFILE=50 stays out of EMFILE
    Given pg_doorman log capture enabled
    And pg_doorman started with NOFILE limit 50 and config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    When we sleep 1000ms
    And we attempt to create 100 idle sessions to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # accept-loop must be rate-limited: a single backoff line every 5 s is
    # acceptable, but tight-loop spam (thousands of lines per ms) is the bug
    # this scenario guards against.
    Then pg_doorman log contains "Failed to accept new connection" at most 5 times
    # Patroni fallback must not be triggered on local fd exhaustion.
    And pg_doorman log does not contain "fallback discovery failed"
    # Binary upgrade must not be aborted by the pre-flight validator just
    # because the local fd table is full — the upgrade is the recovery path.
    And pg_doorman log does not contain "BINARY UPGRADE ABORTED"

  @client-migration @migration-fd-budget
  Scenario: Binary upgrade with 50 idle clients under NOFILE=200 completes without EMFILE
    Given pg_doorman log capture enabled
    And pg_doorman started with NOFILE limit 200 and config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    When we sleep 1000ms
    And we attempt to create 50 idle sessions to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    Then pg_doorman log does not contain "Too many open files"
    And pg_doorman log does not contain "fallback discovery failed"
    And pg_doorman log does not contain "BINARY UPGRADE ABORTED"

  @client-migration @migration-fd-budget @fd-overload
  Scenario: 1000 clients on a 50-client cap reject cleanly, accepted clients keep working
    Given pg_doorman log capture enabled
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      max_connections = 50
      shutdown_timeout = 5000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    When we sleep 1000ms
    And we attempt to create 1000 idle sessions to pg_doorman as "example_user_1" with password "" and database "example_db"
    Then pg_doorman log does not contain "Too many open files"
    And pg_doorman log does not contain "fallback discovery failed"
    When we send SimpleQuery "SELECT 1" to session "idle-0" and store response
    Then session "idle-0" should receive DataRow with "1"
