@go @go-extended
Feature: Go extended protocol tests
  Test pg_doorman extended protocol and batch operations with Go clients

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
      hostnossl all example_user_nopassword 127.0.0.1/32 reject
      hostssl all example_user_nopassword 127.0.0.1/32 trust
      host all example_user_disconnect 127.0.0.1/32 trust
      host all example_user_prometheus 127.0.0.1/32 trust
      host all all 127.0.0.1/32 md5
      host all all 10.0.0.0/8 md5
      host all all 192.168.0.0/16 md5
      host all all 172.0.0.0/8 md5
      """
    And pg_doorman started with config:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      virtual_pool_count = 1
      worker_threads = 2
      prepared_statements = true
      prepared_statements_cache_size = 10000
      tcp_keepalives_idle = 1
      tcp_keepalives_count = 5
      tcp_keepalives_interval = 5
      default_tcp_so_linger = 0
      max_message_size = 1048576
      admin_username = "admin"
      admin_password = "admin"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"
      log_client_parameter_status_changes = true
      idle_timeout = 40000
      application_name = "doorman_example_user_1"

      [pools.example_db.users.0]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 40
      min_pool_size = 0
      pool_mode = "transaction"

      [pools.example_db.users.1]
      username = "example_user_2"
      password = "SCRAM-SHA-256$4096:p2j/1lMdQF6r1dD9I9f7PQ==$H3xt5yh7lwSq9zUPYwHovRu3FyUCCXchG/skydJRa9o=:5xU6Wj/GNg3UnN2uQIx3ezx7uZyzGeM5NrvSJRIxnlw="
      pool_size = 20

      [pools.example_db.users.2]
      username = "example_user_rollback"
      password = "md58b67c8b2b2370f3b5ee2416999588830"
      pool_size = 40
      min_pool_size = 0
      pool_mode = "session"

      [pools.example_db.users.3]
      username = "example_user_nopassword"
      password = ""
      pool_size = 40
      min_pool_size = 0
      pool_mode = "session"

      [pools.example_db.users.4]
      username = "example_user_disconnect"
      password = ""
      pool_size = 40
      min_pool_size = 0
      pool_mode = "transaction"

      [pools.example_db.users.5]
      username = "example_user_prometheus"
      password = ""
      pool_size = 40
      min_pool_size = 0
      pool_mode = "transaction"

      [pools.example_db_alias]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      server_database = "example_db"
      pool_mode = "transaction"
      log_client_parameter_status_changes = true
      idle_timeout = 40000

      [pools.example_db_alias.users.0]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 40
      min_pool_size = 0
      pool_mode = "transaction"
      """

  Scenario: Test extended protocol
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_ExtendedProtocol$
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_ExtendedProtocol"

  Scenario: Test batch operations with sleep
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_SleepBatch
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_SleepBatch"

  Scenario: Test batch operations with errors
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_ErrorBatch
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_ErrorBatch"

  Scenario: Test concurrent extended protocol operations
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_RaceExtendedProtocol
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_RaceExtendedProtocol"

  Scenario: Test disconnect handling
    When I run shell command:
      """
      export DATABASE_URL_DISCONNECT="postgresql://example_user_disconnect@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_Disconnect
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_Disconnect"
