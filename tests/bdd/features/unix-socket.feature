@unix-socket @rust-1
Feature: Unix socket connections

  Scenario: Query via Unix socket reaches PostgreSQL backend
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all   all   trust
      host    all   all   127.0.0.1/32   trust
      """
    And pg_doorman hba file contains:
      """
      local all all trust
      host all all 0.0.0.0/0 trust
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      unix_socket_dir = "${PG_TEMP_DIR}"

      [pools.postgres]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.postgres.users]]
      username = "postgres"
      password = ""
      pool_size = 10
      """
    # pg_backend_pid > 0 proves the query was executed by a real PostgreSQL backend
    Then psql query "SELECT pg_backend_pid() > 0" via pg_doorman unix socket as user "postgres" to database "postgres" returns "t"
