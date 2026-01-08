@bench
Feature: Benchmarking environment setup

  Scenario: Start full stack for benchmarking
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And pg_doorman hba file contains:
      """
      host all all 0.0.0.0/0 trust
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "admin"

      [pools.postgres]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.postgres.users.0]
      username = "postgres"
      password = ""
      pool_size = 20
      """
    And pgbouncer started with config:
      """
      [databases]
      postgres = host=127.0.0.1 port=${PG_PORT} dbname=postgres

      [pgbouncer]
      listen_addr = 127.0.0.1
      listen_port = ${PGBOUNCER_PORT}
      auth_type = trust
      pool_mode = transaction
      max_client_conn = 100
      default_pool_size = 20
      admin_users = postgres
      """
    And odyssey started with config:
      """
      storage "postgres_server" {
        type "remote"
        host "127.0.0.1"
        port ${PG_PORT}
      }

      database "postgres" {
        user "postgres" {
          authentication "none"
          storage "postgres_server"
          pool "transaction"
        }
      }

      listen {
        host "127.0.0.1"
        port ${ODYSSEY_PORT}
      }
      """
