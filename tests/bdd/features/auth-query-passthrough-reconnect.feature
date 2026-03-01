@auth-query @auth-query-passthrough-reconnect
Feature: Auth query passthrough reconnection after pg_terminate_backend

  Tests that after pg_terminate_backend (which triggers mark_bad()),
  a dynamic auth_query passthrough pool can reconnect using the
  cached MD5/SCRAM credentials without re-authenticating the client.

  Scenario: MD5 passthrough reconnection after pg_terminate_backend
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_passthrough_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all postgres 127.0.0.1/32 trust
      host all all 127.0.0.1/32 md5
      """
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
        server_lifetime: 60000
        server_idle_check_timeout: 100
        admin_username: "admin"
        admin_password: "admin"
        tls_private_key: "${DOORMAN_SSL_KEY}"
        tls_certificate: "${DOORMAN_SSL_CERT}"
        pg_hba:
          path: "${DOORMAN_HBA_FILE}"
      pools:
        postgres:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: "transaction"
          users:
            - username: "postgres"
              password: ""
              pool_size: 2
          auth_query:
            query: "SELECT username, password FROM auth_users WHERE username = $1"
            user: "postgres"
            password: ""
            pool_size: 1
            default_pool_size: 5
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    # Create passthrough session and capture backend PID
    When we create session "md5_session" to pg_doorman as "pt_md5_user" with password "md5_pass" and database "postgres"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "md5_session" and store backend_pid as "md5_victim"
    # Create superuser session to terminate the backend
    When we create session "killer" to pg_doorman as "postgres" with password "" and database "postgres"
    # Terminate the backend — connection goes back to pool but is now dead
    And we terminate backend "md5_victim" from session "md5_session" via session "killer"
    # Wait for server_idle_check_timeout to detect the dead connection
    When we sleep for 200 milliseconds
    # Next query should succeed with a new backend connection
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "md5_session" and store backend_pid as "md5_new"
    # Verify we got a different backend (the original was terminated and replaced)
    Then named backend_pid "md5_new" from session "md5_session" is different from "md5_victim"

  Scenario: SCRAM passthrough reconnection after pg_terminate_backend
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            scram-sha-256
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_passthrough_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all postgres 127.0.0.1/32 trust
      host all all 127.0.0.1/32 scram-sha-256
      """
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
        server_lifetime: 60000
        server_idle_check_timeout: 100
        admin_username: "admin"
        admin_password: "admin"
        tls_private_key: "${DOORMAN_SSL_KEY}"
        tls_certificate: "${DOORMAN_SSL_CERT}"
        pg_hba:
          path: "${DOORMAN_HBA_FILE}"
      pools:
        postgres:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: "transaction"
          users:
            - username: "postgres"
              password: ""
              pool_size: 2
          auth_query:
            query: "SELECT username, password FROM auth_users WHERE username = $1"
            user: "postgres"
            password: ""
            pool_size: 1
            default_pool_size: 5
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    # Create passthrough session and capture backend PID
    When we create session "scram_session" to pg_doorman as "pt_scram_user" with password "scram_pass" and database "postgres"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "scram_session" and store backend_pid as "scram_victim"
    # Create superuser session to terminate the backend
    When we create session "killer" to pg_doorman as "postgres" with password "" and database "postgres"
    # Terminate the backend — connection goes back to pool but is now dead
    And we terminate backend "scram_victim" from session "scram_session" via session "killer"
    # Wait for server_idle_check_timeout to detect the dead connection
    When we sleep for 200 milliseconds
    # Next query should succeed with a new backend connection
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "scram_session" and store backend_pid as "scram_new"
    # Verify we got a different backend (the original was terminated and replaced)
    Then named backend_pid "scram_new" from session "scram_session" is different from "scram_victim"
