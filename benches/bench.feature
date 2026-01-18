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

      [[pools.postgres.users]]
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
      log_pooler_errors = 0
      verbose = 0
      log_connections = 0
      log_disconnections = 0
      logfile = /dev/null
      """
    And odyssey started with config:
      """
      workers 4
      log_to_stdout no
      log_file "/dev/null"
      log_format "%p %t %l [%i %s] (%c) %m\n"
      log_debug no
      log_config no
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
          pool_discard no
          pool_reserve_prepared_statement yes
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

    # ==================== SIMPLE PROTOCOL ====================

    # --- 1 client, simple protocol ---
    When I run pgbench for "pg_doorman_simple_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_simple_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_simple_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "simple_c1" to bencher

    # --- 40 clients, simple protocol ---
    When I run pgbench for "pg_doorman_simple_c40" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_simple_c40" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_simple_c40" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "simple_c40" to bencher

    # --- 80 clients, simple protocol ---
    When I run pgbench for "pg_doorman_simple_c80" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_simple_c80" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_simple_c80" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "simple_c80" to bencher

    # --- 120 clients, simple protocol ---
    When I run pgbench for "pg_doorman_simple_c120" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_simple_c120" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_simple_c120" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "simple_c120" to bencher

    # ==================== EXTENDED PROTOCOL ====================

    # --- 1 client, extended protocol ---
    When I run pgbench for "pg_doorman_extended_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_extended_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_extended_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "extended_c1" to bencher

    # --- 40 clients, extended protocol ---
    When I run pgbench for "pg_doorman_extended_c40" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_extended_c40" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_extended_c40" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "extended_c40" to bencher

    # --- 80 clients, extended protocol ---
    When I run pgbench for "pg_doorman_extended_c80" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_extended_c80" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_extended_c80" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "extended_c80" to bencher

    # --- 120 clients, extended protocol ---
    When I run pgbench for "pg_doorman_extended_c120" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_extended_c120" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_extended_c120" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "extended_c120" to bencher

    # ==================== PREPARED PROTOCOL ====================

    # --- 1 client, prepared protocol ---
    When I run pgbench for "pg_doorman_prepared_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_prepared_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_prepared_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "prepared_c1" to bencher

    # --- 40 clients, prepared protocol ---
    When I run pgbench for "pg_doorman_prepared_c40" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_prepared_c40" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_prepared_c40" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "prepared_c40" to bencher

    # --- 80 clients, prepared protocol ---
    When I run pgbench for "pg_doorman_prepared_c80" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_prepared_c80" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_prepared_c80" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "prepared_c80" to bencher

    # --- 120 clients, prepared protocol ---
    When I run pgbench for "pg_doorman_prepared_c120" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_prepared_c120" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_prepared_c120" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "prepared_c120" to bencher

    # ==================== WITH --connect (reconnect each transaction) ====================

    # --- 1 client, simple protocol, with connect ---
    When I run pgbench for "pg_doorman_simple_connect_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_simple_connect_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_simple_connect_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "simple_connect_c1" to bencher

    # --- 40 clients, simple protocol, with connect ---
    When I run pgbench for "pg_doorman_simple_connect_c40" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_simple_connect_c40" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_simple_connect_c40" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "simple_connect_c40" to bencher

    # --- 80 clients, simple protocol, with connect ---
    When I run pgbench for "pg_doorman_simple_connect_c80" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_simple_connect_c80" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_simple_connect_c80" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "simple_connect_c80" to bencher

    # --- 120 clients, simple protocol, with connect ---
    When I run pgbench for "pg_doorman_simple_connect_c120" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_simple_connect_c120" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_simple_connect_c120" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "simple_connect_c120" to bencher

    # --- 1 client, extended protocol, with connect ---
    When I run pgbench for "pg_doorman_extended_connect_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_extended_connect_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_extended_connect_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "extended_connect_c1" to bencher

    # --- 40 clients, extended protocol, with connect ---
    When I run pgbench for "pg_doorman_extended_connect_c40" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_extended_connect_c40" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_extended_connect_c40" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "extended_connect_c40" to bencher

    # --- 80 clients, extended protocol, with connect ---
    When I run pgbench for "pg_doorman_extended_connect_c80" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_extended_connect_c80" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_extended_connect_c80" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "extended_connect_c80" to bencher

    # --- 120 clients, extended protocol, with connect ---
    When I run pgbench for "pg_doorman_extended_connect_c120" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_extended_connect_c120" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_extended_connect_c120" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=extended --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "extended_connect_c120" to bencher

    # --- 1 client, prepared protocol, with connect ---
    When I run pgbench for "pg_doorman_prepared_connect_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_prepared_connect_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_prepared_connect_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "prepared_connect_c1" to bencher

    # --- 40 clients, prepared protocol, with connect ---
    When I run pgbench for "pg_doorman_prepared_connect_c40" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_prepared_connect_c40" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_prepared_connect_c40" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "prepared_connect_c40" to bencher

    # --- 80 clients, prepared protocol, with connect ---
    When I run pgbench for "pg_doorman_prepared_connect_c80" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_prepared_connect_c80" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_prepared_connect_c80" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "prepared_connect_c80" to bencher

    # --- 120 clients, prepared protocol, with connect ---
    When I run pgbench for "pg_doorman_prepared_connect_c120" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "odyssey_prepared_connect_c120" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I run pgbench for "pgbouncer_prepared_connect_c120" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=prepared --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=disable"
    When I send benchmark results for "prepared_connect_c120" to bencher

    # ==================== SSL + SIMPLE PROTOCOL ====================

    # --- 1 client, simple protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_simple_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_simple_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_simple_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_simple_c1" to bencher

    # --- 40 clients, simple protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_simple_c40" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_simple_c40" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_simple_c40" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_simple_c40" to bencher

    # --- 80 clients, simple protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_simple_c80" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_simple_c80" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_simple_c80" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_simple_c80" to bencher

    # --- 120 clients, simple protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_simple_c120" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_simple_c120" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_simple_c120" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_simple_c120" to bencher

    # ==================== SSL + EXTENDED PROTOCOL ====================

    # --- 1 client, extended protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_extended_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_extended_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_extended_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_extended_c1" to bencher

    # --- 40 clients, extended protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_extended_c40" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_extended_c40" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_extended_c40" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_extended_c40" to bencher

    # --- 80 clients, extended protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_extended_c80" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_extended_c80" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_extended_c80" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_extended_c80" to bencher

    # --- 120 clients, extended protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_extended_c120" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_extended_c120" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_extended_c120" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_extended_c120" to bencher

    # ==================== SSL + PREPARED PROTOCOL ====================

    # --- 1 client, prepared protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_prepared_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_prepared_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_prepared_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_prepared_c1" to bencher

    # --- 40 clients, prepared protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_prepared_c40" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_prepared_c40" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_prepared_c40" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_prepared_c40" to bencher

    # --- 80 clients, prepared protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_prepared_c80" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_prepared_c80" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_prepared_c80" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_prepared_c80" to bencher

    # --- 120 clients, prepared protocol, SSL ---
    When I run pgbench for "pg_doorman_ssl_prepared_c120" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_prepared_c120" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_prepared_c120" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=prepared postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_prepared_c120" to bencher

    # ==================== SSL + CONNECT ====================

    # --- 1 client, simple protocol, SSL, with connect ---
    When I run pgbench for "pg_doorman_ssl_connect_c1" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_connect_c1" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_connect_c1" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 1 -j 1 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_connect_c1" to bencher

    # --- 40 clients, simple protocol, SSL, with connect ---
    When I run pgbench for "pg_doorman_ssl_connect_c40" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_connect_c40" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_connect_c40" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 40 -j 2 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_connect_c40" to bencher

    # --- 80 clients, simple protocol, SSL, with connect ---
    When I run pgbench for "pg_doorman_ssl_connect_c80" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_connect_c80" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_connect_c80" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 80 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_connect_c80" to bencher

    # --- 120 clients, simple protocol, SSL, with connect ---
    When I run pgbench for "pg_doorman_ssl_connect_c120" with "-n -h 127.0.0.1 -p ${DOORMAN_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "odyssey_ssl_connect_c120" with "-n -h 127.0.0.1 -p ${ODYSSEY_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I run pgbench for "pgbouncer_ssl_connect_c120" with "-n -h 127.0.0.1 -p ${PGBOUNCER_PORT} -U postgres -c 120 -j 4 -T 30 -P 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}" and env "PGSSLMODE=require"
    When I send benchmark results for "ssl_connect_c120" to bencher

    # Print and send results
    Then I print benchmark results
    And I send normalized benchmark results to bencher.dev
    And I generate benchmark markdown table
