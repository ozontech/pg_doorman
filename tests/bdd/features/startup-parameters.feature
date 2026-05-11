@startup-parameters
Feature: Per-pool startup_parameters injection
  pg_doorman injects operator-supplied PostgreSQL run-time parameters into
  every backend StartupMessage on a three-level cascade: general defaults,
  per-pool overrides, and (in auth_query passthrough mode) per-user
  overrides from an optional JSON column.

  Scenario: general.startup_parameters apply on a fresh backend
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [general.startup_parameters]
      statement_timeout = "12345"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 2
      """
    Then psql query "SHOW statement_timeout" via pg_doorman as user "example_user_1" to database "example_db" with password "test" returns "12345ms"

  Scenario: pool.startup_parameters overrides general per-key
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [general.startup_parameters]
      statement_timeout = "10001"
      lock_timeout = "5001"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.example_db.startup_parameters]
      statement_timeout = "23456"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 2
      """
    Then psql query "SHOW statement_timeout" via pg_doorman as user "example_user_1" to database "example_db" with password "test" returns "23456ms"
    And psql query "SHOW lock_timeout" via pg_doorman as user "example_user_1" to database "example_db" with password "test" returns "5001ms"

  Scenario: auth_query passthrough per-user JSON column overrides pool default
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_startup_params_fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [pools.postgres]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.postgres.startup_parameters]
      plan_cache_mode = "auto"

      [pools.postgres.auth_query]
      query = "SELECT username, password, startup_parameters FROM auth_users WHERE username = $1"
      user = "postgres"
      password = ""
      workers = 1
      pool_size = 5
      cache_ttl = "1h"
      cache_failure_ttl = "30s"
      min_interval = "0s"
      """
    Then psql query "SHOW plan_cache_mode" via pg_doorman as user "sp_tuned_user" to database "postgres" with password "tuned_pass" returns "force_custom_plan"
    And psql query "SHOW plan_cache_mode" via pg_doorman as user "sp_plain_user" to database "postgres" with password "plain_pass" returns "auto"

  Scenario: operator-supplied application_name in startup_parameters wins over pool default
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"
      application_name = "doorman_default_app"

      [pools.example_db.startup_parameters]
      application_name = "operator_supplied_app"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 2
      """
    Then psql query "SHOW application_name" via pg_doorman as user "example_user_1" to database "example_db" with password "test" returns "operator_supplied_app"

  Scenario: reserved-key in startup_parameters is rejected at config validation
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 2
      """
    # pg_doorman is running with a valid config. Now overwrite the file with a
    # reserved-key violation and re-run the binary in -t mode to confirm config
    # validation rejects the change before any worker can pick it up.
    When we overwrite pg_doorman config file with:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.example_db.startup_parameters]
      user = "evil_override"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 2
      """
    # `pg_doorman -t` exits non-zero when validation fails; the precise
    # reserved-key message lands in a logger that has not been initialised at
    # this point in startup, so we assert on the exit code only. The narrower
    # reserved-key behaviour is exercised by unit tests in src/config/tests.rs.
    And I run shell command "${DOORMAN_BINARY} -t ${DOORMAN_CONFIG_FILE}"
    Then the command should fail

  Scenario: invalid GUC name produces a backend startup error with warn log
    Given pg_doorman log capture enabled
    And PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"
      startup_parameter_quarantine_threshold = 999

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.example_db.startup_parameters]
      nonexistent_guc_zzz = "value"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 2
      """
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "test" fails
    And pg_doorman log contains "backend startup rejected"
    And pg_doorman log contains "nonexistent_guc_zzz"

  Scenario: quarantine after consecutive rejections strips the bad key on next attempt
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"
      startup_parameter_quarantine_threshold = 1
      startup_parameter_quarantine_ttl = 60000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.example_db.startup_parameters]
      nonexistent_guc_yyy = "value"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 2
      """
    # First connection trips the quarantine threshold and fails.
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "test" fails
    # After quarantine engages, the bad key is stripped and a fresh backend can
    # be created; the connection now succeeds and the SHOW returns the default
    # because nonexistent_guc_yyy never reached PostgreSQL.
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "test" succeeds
    # SHOW POOLS surfaces the quarantined parameter name so an operator
    # triaging from psql can see what pg_doorman is parking.
    When we create admin session "adm" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "adm" and store response
    Then admin session "adm" response should contain "nonexistent_guc_yyy"

  Scenario: RESET ALL restores startup_parameters defaults
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "session"

      [pools.example_db.startup_parameters]
      plan_cache_mode = "force_custom_plan"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 2
      """
    When I run shell command:
      """
      PGPASSWORD=test PGSSLMODE=disable psql -h 127.0.0.1 -p ${DOORMAN_PORT} \
        -U example_user_1 -d example_db -A -t <<'SQL'
      SET plan_cache_mode = 'auto';
      SHOW plan_cache_mode;
      RESET ALL;
      SHOW plan_cache_mode;
      SQL
      """
    Then the command should succeed
    # The first SHOW returns the client-set value, the second returns the
    # operator-supplied startup default (RESET ALL falls back to reset_val,
    # which is the value PG saw in StartupMessage).
    And the command output should contain "auto"
    And the command output should contain "force_custom_plan"

  Scenario: DISCARD ALL restores startup_parameters defaults
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "session"

      [pools.example_db.startup_parameters]
      plan_cache_mode = "force_custom_plan"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 2
      """
    When I run shell command:
      """
      PGPASSWORD=test PGSSLMODE=disable psql -h 127.0.0.1 -p ${DOORMAN_PORT} \
        -U example_user_1 -d example_db -A -t <<'SQL'
      SET plan_cache_mode = 'auto';
      SHOW plan_cache_mode;
      DISCARD ALL;
      SHOW plan_cache_mode;
      SQL
      """
    Then the command should succeed
    And the command output should contain "auto"
    And the command output should contain "force_custom_plan"

  Scenario: RELOAD recycles pool when pool.startup_parameters changes
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.example_db.startup_parameters]
      statement_timeout = "11111"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 2
      """
    Then psql query "SHOW statement_timeout" via pg_doorman as user "example_user_1" to database "example_db" with password "test" returns "11111ms"
    When we overwrite pg_doorman config file with:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.example_db.startup_parameters]
      statement_timeout = "22222"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 2
      """
    And we create admin session "adm1" to pg_doorman as "admin" with password "admin"
    And we execute "RELOAD" on admin session "adm1"
    And we sleep for 500 milliseconds
    Then psql query "SHOW statement_timeout" via pg_doorman as user "example_user_1" to database "example_db" with password "test" returns "22222ms"

  Scenario: dedicated auth_query mode logs a warning and ignores per-user startup_parameters
    Given pg_doorman log capture enabled
    And PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_startup_params_fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [pools.postgres]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.postgres.auth_query]
      query = "SELECT username, password, startup_parameters FROM auth_users WHERE username = $1"
      user = "postgres"
      password = ""
      workers = 1
      pool_size = 5
      cache_ttl = "1h"
      cache_failure_ttl = "30s"
      min_interval = "0s"
      server_user = "postgres"
      server_password = ""
      """
    # Authentication still succeeds; the JSON-column value is dropped silently
    # at the cache layer in dedicated mode because every dynamic user shares
    # a single backend identity. The warning surfaces the dropped key for the
    # operator instead of leaving it unobservable.
    Then psql connection to pg_doorman as user "sp_tuned_user" to database "postgres" with password "tuned_pass" succeeds
    And pg_doorman log contains "per-user startup_parameters ignored in dedicated"

  Scenario: auth_query SQL without the startup_parameters column keeps working
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_passthrough_fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 md5"

      [pools.postgres]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.postgres.auth_query]
      query = "SELECT username, password FROM auth_users WHERE username = $1"
      user = "postgres"
      password = ""
      workers = 1
      pool_size = 5
      cache_ttl = "1h"
      cache_failure_ttl = "30s"
      min_interval = "0s"
      """
    Then psql connection to pg_doorman as user "pt_md5_user" to database "postgres" with password "md5_pass" succeeds
