@rust @rust-4 @thread-per-core-pool
Feature: Thread-per-core backend locality
  Verify deterministic backend movement between fixed worker runtimes.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @local-global-worker-migration
  Scenario: Local idle backend moves through global queue to the other worker
    Given pg_doorman log capture enabled
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      worker_threads = 2
      thread_per_core = true
      worker_cpu_affinity_pinning = false

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    # Keeping the owner session open makes the next client select worker 1.
    When we create session "owner" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "owner" and store backend_pid

    # The fixed local-idle grace is 500ms. The backend must now be global.
    When we sleep for 750 milliseconds
    Then pg_doorman log contains "local idle published to global:"

    # The other worker has no local backend and pool_size=1 prevents another.
    When we create session "borrower" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "borrower" and store backend_pid
    Then backend_pid from session "owner" should equal backend_pid from session "borrower"
    And pg_doorman log shows backend migration between two workers
