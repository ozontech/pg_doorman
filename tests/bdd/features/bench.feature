@bench
Feature: Benchmarking environment setup with SSL

  Scenario: Run pgbench against all connection poolers (with and without SSL)
    Given PostgreSQL started with options "-c max_connections=500" and pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And self-signed SSL certificates are generated
    And pgbench script file:
      """
      \set aid random(1, 100000)
      select :aid;
      """
    And pg_doorman hba file contains:
      """
      host all all 0.0.0.0/0 trust
      hostssl all all 0.0.0.0/0 trust
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      worker_threads = 4
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "admin"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.postgres]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.postgres.users.0]
      username = "postgres"
      password = ""
      pool_size = 40
      """
    And pgbouncer userlist file:
      """
      "postgres" ""
      """
    And pgbouncer started with config:
      """
      [databases]
      postgres = host=127.0.0.1 port=${PG_PORT} dbname=postgres

      [pgbouncer]
      listen_addr = 127.0.0.1
      listen_port = ${PGBOUNCER_PORT}
      unix_socket_dir =
      auth_type = trust
      auth_file = ${PGBOUNCER_USERLIST}
      pool_mode = transaction
      max_client_conn = 300
      default_pool_size = 40
      admin_users = postgres
      client_tls_sslmode = allow
      client_tls_key_file = ${DOORMAN_SSL_KEY}
      client_tls_cert_file = ${DOORMAN_SSL_CERT}
      client_tls_ca_file = ${DOORMAN_SSL_CERT}
      """
    And odyssey started with config:
      """
      workers 4
      log_to_stdout no
      log_format "%p %t %l [%i %s] (%c) %m\n"
      log_debug no
      log_config yes
      log_session no
      log_query no
      log_stats no

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
          pool_size 40
        }
      }

      listen {
        host "127.0.0.1"
        port ${ODYSSEY_PORT}
        tls "allow"
        tls_cert_file "${DOORMAN_SSL_CERT}"
        tls_key_file "${DOORMAN_SSL_KEY}"
      }
      """

    # ==================== NON-SSL BENCHMARKS ====================

    # --- 1 client ---
    When I run pgbench for "postgresql_c1" with "-n -h 127.0.0.1 -p ${PG_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "pg_doorman_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "odyssey_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "pgbouncer_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"

    # --- 10 clients ---
    When I run pgbench for "postgresql_c10" with "-n -h 127.0.0.1 -p ${PG_PORT} -U postgres -c 10 -j 2 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "pg_doorman_c10" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 10 -j 2 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "odyssey_c10" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 10 -j 2 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "pgbouncer_c10" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 10 -j 2 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"

    # --- 50 clients ---
    When I run pgbench for "postgresql_c50" with "-n -h 127.0.0.1 -p ${PG_PORT} -U postgres -c 50 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "pg_doorman_c50" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 50 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "odyssey_c50" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 50 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "pgbouncer_c50" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 50 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"

    # --- 100 clients ---
    When I run pgbench for "postgresql_c100" with "-n -h 127.0.0.1 -p ${PG_PORT} -U postgres -c 100 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "pg_doorman_c100" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 100 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "odyssey_c100" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 100 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "pgbouncer_c100" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 100 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"

    # --- 200 clients ---
    When I run pgbench for "postgresql_c200" with "-n -h 127.0.0.1 -p ${PG_PORT} -U postgres -c 200 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "pg_doorman_c200" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 200 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "odyssey_c200" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 200 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"
    When I run pgbench for "pgbouncer_c200" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 200 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}"

    # ==================== SSL BENCHMARKS ====================

    # --- 1 client SSL ---
    When I run pgbench for "pg_doorman_ssl_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"

    # --- 10 clients SSL ---
    When I run pgbench for "pg_doorman_ssl_c10" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 10 -j 2 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_c10" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 10 -j 2 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_c10" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 10 -j 2 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"

    # --- 50 clients SSL ---
    When I run pgbench for "pg_doorman_ssl_c50" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 50 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_c50" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 50 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_c50" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 50 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"

    # --- 100 clients SSL ---
    When I run pgbench for "pg_doorman_ssl_c100" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 100 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_c100" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 100 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_c100" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 100 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"

    # --- 200 clients SSL ---
    When I run pgbench for "pg_doorman_ssl_c200" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 200 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_c200" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 200 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_c200" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 200 -j 4 -T 30 -P 1 postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"

    # Print and send results
    Then I print benchmark results
    And I send normalized benchmark results to bencher.dev
