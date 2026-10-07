@rust @rust-3 @cleanup-review @cleanup-temp-tables
Feature: Adaptive cleanup removes temporary tables left by a client
  The built-in cleanup tracks the CREATE TABLE command tag and sends
  DISCARD TEMP at checkin. `SELECT ... INTO TEMP` and `CREATE TEMP TABLE
  AS SELECT` complete with the inner query's tag on current PostgreSQL
  and are not tracked. Permanent objects created by CREATE TABLE are not
  dropped: DISCARD TEMP only touches temporary objects.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @cleanup-temp-tables-transaction
  Scenario: A temp table does not survive a checkin in transaction mode
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid
    And we send SimpleQuery "CREATE TEMP TABLE leak_a (i int)" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "old"
    And we send SimpleQuery "SELECT to_regclass('pg_temp.leak_a') IS NULL" to session "next" and store response
    Then session "next" should receive DataRow with "t"
    And session "next" should receive ReadyForQuery "I"

  @cleanup-temp-tables-session
  Scenario: A temp table does not survive a checkin in session mode
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: session
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid
    And we send SimpleQuery "CREATE TEMP TABLE leak_session (i int)" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "old"
    And we send SimpleQuery "SELECT to_regclass('pg_temp.leak_session') IS NULL" to session "next" and store response
    Then session "next" should receive DataRow with "t"

  @cleanup-temp-tables-permanent
  Scenario: A permanent table survives the temp-cleanup checkin
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      web:
        enabled: true
        host: "127.0.0.1"
        port: 9129
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid
    And we send SimpleQuery "CREATE TABLE leak_perm (i int)" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "old"
    # The CREATE TABLE tag arms the temp cleanup; DISCARD TEMP must not touch
    # the permanent table while the checkin itself becomes observable.
    And we send SimpleQuery "SELECT to_regclass('public.leak_perm') IS NOT NULL" to session "next" and store response
    Then session "next" should receive DataRow with "t"
    When I run shell command:
      """
      python3 - <<'PY'
      import time
      import urllib.request
      deadline = time.monotonic() + 5
      while True:
          body = urllib.request.urlopen('http://127.0.0.1:9129/metrics', timeout=2).read().decode()
          lines = body.splitlines()
          if 'pg_doorman_server_cleanup_total{database="example_db",result="ok",user="example_user_1"} 1' in lines:
              break
          assert time.monotonic() < deadline, body
          time.sleep(0.05)
      PY
      """
    Then the command should succeed

  @cleanup-temp-tables-readonly
  Scenario: Read-only clients do not trigger any cleanup
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      web:
        enabled: true
        host: "127.0.0.1"
        port: 9129
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "next"
    And I run shell command:
      """
      python3 - <<'PY'
      import time
      import urllib.request
      deadline = time.monotonic() + 5
      while True:
          body = urllib.request.urlopen('http://127.0.0.1:9129/metrics', timeout=2).read().decode()
          lines = body.splitlines()
          ok = 'pg_doorman_server_cleanup_total{database="example_db",result="ok",user="example_user_1"} 0' in lines
          err = 'pg_doorman_server_cleanup_total{database="example_db",result="error",user="example_user_1"} 0' in lines
          if ok and err:
              break
          assert time.monotonic() < deadline, body
          time.sleep(0.05)
      PY
      """
    Then the command should succeed

  @cleanup-temp-tables-advisory
  Scenario: The cleanup batch releases session-level advisory locks
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid
    # The SET arms the built-in cleanup batch; the advisory lock has no
    # command tag and rides on the batch via pg_advisory_unlock_all().
    And we send SimpleQuery "SELECT pg_advisory_lock(424242)" to session "old"
    And we send SimpleQuery "SET statement_timeout = 10000" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "old"
    And we send SimpleQuery "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory'" to session "next" and store response
    Then session "next" should receive DataRow with "0"

  @cleanup-temp-tables-advisory-lone
  Scenario: A lone advisory lock survives the checkin
    # The lock has no command tag: it does not arm the cleanup batch, and no
    # other statement arms it either. No batch runs, and the lock reaches the
    # next client on the same backend. This is the documented hole in
    # "Not tracked": release such locks in the application, or use `always`.
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      web:
        enabled: true
        host: "127.0.0.1"
        port: 9129
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid
    # pg_advisory_lock answers with the plain SELECT tag: nothing is armed.
    And we send SimpleQuery "SELECT pg_advisory_lock(10, 20)" to session "old"
    And we send SimpleQuery "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND classid = 10 AND objid = 20" to session "old" and store response
    Then session "old" should receive DataRow with "1"
    When we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "old"
    # The lock outlived the client that took it.
    And we send SimpleQuery "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND classid = 10 AND objid = 20" to session "next" and store response
    Then session "next" should receive DataRow with "1"
    # No cleanup batch ran: both counters stayed at zero.
    When I run shell command:
      """
      python3 - <<'PY'
      import time
      import urllib.request
      deadline = time.monotonic() + 5
      while True:
          body = urllib.request.urlopen('http://127.0.0.1:9129/metrics', timeout=2).read().decode()
          lines = body.splitlines()
          ok = 'pg_doorman_server_cleanup_total{database="example_db",result="ok",user="example_user_1"} 0' in lines
          err = 'pg_doorman_server_cleanup_total{database="example_db",result="error",user="example_user_1"} 0' in lines
          if ok and err:
              break
          assert time.monotonic() < deadline, body
          time.sleep(0.05)
      PY
      """
    Then the command should succeed
