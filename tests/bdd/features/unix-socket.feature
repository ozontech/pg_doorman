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

  Scenario: Unix socket file gets the default 0600 permission bits
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
    Then pg_doorman unix socket file has mode "0600"

  Scenario: Unix socket file honours configured unix_socket_mode
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
      unix_socket_mode = "0660"

      [pools.postgres]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.postgres.users]]
      username = "postgres"
      password = ""
      pool_size = 10
      """
    Then pg_doorman unix socket file has mode "0660"

  Scenario: HBA local reject blocks Unix socket connection
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all   all   trust
      host    all   all   127.0.0.1/32   trust
      """
    And pg_doorman hba file contains:
      """
      local all all reject
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
    Then psql connection to pg_doorman via unix socket as user "postgres" to database "postgres" fails

  Scenario: HBA host rule does not match Unix socket connection
    # Only a `host` rule for 127.0.0.1 — Unix clients have no peer IP, so
    # this rule must NOT authenticate them. Without a `local` rule the
    # connection should be rejected by the matcher.
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all   all   trust
      host    all   all   127.0.0.1/32   trust
      """
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 trust
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
    Then psql connection to pg_doorman via unix socket as user "postgres" to database "postgres" fails

  Scenario: only_ssl_connections does not block Unix socket clients
    # only_ssl_connections rejects plain TCP, but Unix sockets are inherently
    # local-only and should never be subject to the TLS-required check.
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all   all   trust
      host    all   all   127.0.0.1/32   trust
      """
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      local all all trust
      hostssl all all 0.0.0.0/0 trust
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"
      tls_mode = "require"
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
    Then psql query "SELECT 1" via pg_doorman unix socket as user "postgres" to database "postgres" returns "1"
