@pooler-check-query-cache
Feature: pooler_check_query response cache
  The first SimpleQuery matching general.pooler_check_query in a pool's
  lifetime is forwarded to PostgreSQL. The response is cached per pool
  and subsequent matching queries are served from cache without touching
  the backend. Cache invalidates when general.pooler_check_query changes
  via RELOAD. ErrorResponse from backend is never cached.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman hba file contains:
      """
      host all admin 127.0.0.1/32 trust
      host all example_user_1 127.0.0.1/32 md5
      """
    And self-signed SSL certificates are generated

  Scenario: default ";" — first ping hits backend, second hit served from cache
    Given pg_doorman started with config:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "admin"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 4
      """
    When I run shell command:
      """
      export PGPASSWORD=test
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c ";" >/dev/null
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c ";" >/dev/null

      BACKEND=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_backend_total / {print $2; f=1} END {if (!f) print 0}')
      CACHE=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_cache_total / {print $2; f=1} END {if (!f) print 0}')

      echo "backend=$BACKEND cache=$CACHE"
      test "$BACKEND" = "1" || { echo "expected backend_total=1, got $BACKEND"; exit 1; }
      test "$CACHE"   = "1" || { echo "expected cache_total=1, got $CACHE"; exit 1; }
      """
    Then the command should succeed

  Scenario: custom "select 1" — first ping hits backend, second hit served from cache
    Given pg_doorman started with config:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "admin"
      pooler_check_query = "select 1"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 4
      """
    When I run shell command:
      """
      export PGPASSWORD=test
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select 1" >/dev/null
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select 1" >/dev/null
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select 1" >/dev/null

      BACKEND=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_backend_total / {print $2; f=1} END {if (!f) print 0}')
      CACHE=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_cache_total / {print $2; f=1} END {if (!f) print 0}')

      echo "backend=$BACKEND cache=$CACHE"
      test "$BACKEND" = "1" || { echo "expected backend_total=1, got $BACKEND"; exit 1; }
      test "$CACHE"   = "2" || { echo "expected cache_total=2, got $CACHE"; exit 1; }
      """
    Then the command should succeed

  Scenario: RELOAD with a different pooler_check_query value invalidates the cache
    Given pg_doorman started with config:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "admin"
      pooler_check_query = "select 1"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 4
      """
    When we create admin session "adm" to pg_doorman as "admin" with password "admin"
    And I run shell command:
      """
      export PGPASSWORD=test
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select 1" >/dev/null
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select 1" >/dev/null
      """
    Then the command should succeed
    When we overwrite pg_doorman config file with:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "admin"
      pooler_check_query = "select 2"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 4
      """
    And we execute "RELOAD" on admin session "adm" and store response
    And we sleep for 500 milliseconds
    And I run shell command:
      """
      export PGPASSWORD=test
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select 2" >/dev/null
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select 2" >/dev/null

      BACKEND=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_backend_total / {print $2; f=1} END {if (!f) print 0}')
      CACHE=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_cache_total / {print $2; f=1} END {if (!f) print 0}')

      echo "backend=$BACKEND cache=$CACHE"
      test "$BACKEND" = "2" || { echo "expected backend_total=2 (one before RELOAD, one after), got $BACKEND"; exit 1; }
      test "$CACHE"   = "2" || { echo "expected cache_total=2 (one before RELOAD, one after), got $CACHE"; exit 1; }
      """
    Then the command should succeed

  Scenario: backend ErrorResponse is forwarded but never cached
    Given pg_doorman started with config:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "admin"
      pooler_check_query = "select * from no_such_table_aaa"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 4
      """
    When I run shell command:
      """
      export PGPASSWORD=test
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select * from no_such_table_aaa" 2>&1 | grep -q "no_such_table_aaa" || { echo "first probe must surface the backend error"; exit 1; }
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select * from no_such_table_aaa" 2>&1 | grep -q "no_such_table_aaa" || { echo "second probe must also reach backend, not cache"; exit 1; }

      BACKEND=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_backend_total / {print $2; f=1} END {if (!f) print 0}')
      CACHE=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_cache_total / {print $2; f=1} END {if (!f) print 0}')

      echo "backend=$BACKEND cache=$CACHE"
      test "$BACKEND" = "2" || { echo "expected backend_total=2 (errors are never cached), got $BACKEND"; exit 1; }
      test "$CACHE"   = "0" || { echo "expected cache_total=0, got $CACHE"; exit 1; }
      """
    Then the command should succeed

  Scenario: response larger than the per-message buffer is drained, cached, and the pool stays in sync
    Given pg_doorman started with config:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "admin"
      pooler_check_query = "select repeat('x', 10000)"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 4
      """
    When I run shell command:
      """
      export PGPASSWORD=test

      LEN1=$(psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -t -A -c "select repeat('x', 10000)" | tr -d '\n' | wc -c)
      LEN2=$(psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -t -A -c "select repeat('x', 10000)" | tr -d '\n' | wc -c)
      LEN3=$(psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -t -A -c "select repeat('x', 10000)" | tr -d '\n' | wc -c)

      test "$LEN1" = "10000" || { echo "first probe truncated: $LEN1 (expected 10000)"; exit 1; }
      test "$LEN2" = "10000" || { echo "second probe truncated: $LEN2 (expected 10000)"; exit 1; }
      test "$LEN3" = "10000" || { echo "third probe truncated: $LEN3 (expected 10000)"; exit 1; }

      OTHER=$(psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -t -A -c "select 42" | head -1)
      test "$OTHER" = "42" || { echo "regular query after large check_query desynced: got '$OTHER'"; exit 1; }

      BACKEND=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_backend_total / {print $2; f=1} END {if (!f) print 0}')
      CACHE=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_cache_total / {print $2; f=1} END {if (!f) print 0}')

      echo "backend=$BACKEND cache=$CACHE"
      test "$BACKEND" = "1" || { echo "expected backend_total=1 (large response cached after drain), got $BACKEND"; exit 1; }
      test "$CACHE"   = "2" || { echo "expected cache_total=2, got $CACHE"; exit 1; }
      """
    Then the command should succeed

  Scenario: bytes from the previous pooler_check_query stop matching after RELOAD
    Given pg_doorman started with config:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "admin"
      pooler_check_query = "select 1"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 4
      """
    When we create admin session "adm" to pg_doorman as "admin" with password "admin"
    And I run shell command:
      """
      export PGPASSWORD=test
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select 1" >/dev/null
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select 1" >/dev/null
      """
    Then the command should succeed
    When we overwrite pg_doorman config file with:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "admin"
      pooler_check_query = "select 2"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 4
      """
    And we execute "RELOAD" on admin session "adm" and store response
    And we sleep for 500 milliseconds
    And I run shell command:
      """
      export PGPASSWORD=test

      # "select 1" no longer matches the active probe — the snapshot now holds
      # "select 2". The bytes go through the normal SQL path and reach the
      # backend as a regular query; check_query counters must not move.
      OUT=$(psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -t -A -c "select 1" | tr -d '\n')
      test "$OUT" = "1" || { echo "regular SQL path for stale probe returned '$OUT', expected '1'"; exit 1; }

      # The new probe value must now run the cache flow.
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select 2" >/dev/null
      psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U example_user_1 -d example_db -c "select 2" >/dev/null

      BACKEND=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_backend_total / {print $2; f=1} END {if (!f) print 0}')
      CACHE=$(curl -s http://127.0.0.1:9127/metrics | awk '/^pg_doorman_pooler_check_query_cache_total / {print $2; f=1} END {if (!f) print 0}')

      echo "backend=$BACKEND cache=$CACHE"
      # backend_total: 1 from pre-RELOAD select 1, 1 from first post-RELOAD select 2 = 2
      # cache_total: 1 pre-RELOAD select 1 hit, 1 post-RELOAD select 2 hit = 2
      # The stale "select 1" after RELOAD goes through normal SQL, not check_query.
      test "$BACKEND" = "2" || { echo "expected backend_total=2, got $BACKEND"; exit 1; }
      test "$CACHE"   = "2" || { echo "expected cache_total=2, got $CACHE"; exit 1; }
      """
    Then the command should succeed
