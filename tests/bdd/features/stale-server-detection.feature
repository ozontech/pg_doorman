@rust @rust-4 @stale-server-detection
Feature: Detect dead server connections while client is idle in transaction
  When a PostgreSQL backend is terminated (e.g., via pg_terminate_backend)
  while a client is idle inside a transaction, pg_doorman should detect the
  dead connection promptly and send an ErrorResponse to the client instead
  of hanging indefinitely.

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

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """

  @stale-server-idle-in-tx
  Scenario: Client idle in transaction receives error when backend is terminated
    # 1. Start a transaction on main session
    When we create session "main" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "main"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "main" and store backend_pid

    # 2. Create killer session and terminate the backend
    When we create session "killer" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we terminate backend of session "main" via session "killer"

    # 3. Small delay for termination to propagate
    And we sleep 200ms

    # 4. Try to query on main session - should get error (connection was killed)
    When we send SimpleQuery "SELECT 1" to session "main" expecting connection close
