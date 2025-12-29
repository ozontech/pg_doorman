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
      host all example_user_disconnect 127.0.0.1/32 trust
      host all all 127.0.0.1/32 md5
      """
    And self-signed SSL certificates are generated
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      prepared_statements = true
      prepared_statements_cache_size = 10000
      admin_username = "admin"
      admin_password = "admin"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.example_db.users.0]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 40

      [pools.example_db.users.1]
      username = "example_user_disconnect"
      password = ""
      pool_size = 40
      """

  Scenario: Test extended protocol
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      export PG_PORT="${PG_PORT}"
      cd tests/go && go test -v -run Test_ExtendedProtocol$ ./extended
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_ExtendedProtocol"

  Scenario: Test batch operations with sleep
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_SleepBatch ./extended
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_SleepBatch"

  Scenario: Test batch operations with errors
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_ErrorBatch ./extended
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_ErrorBatch"

  Scenario: Test concurrent extended protocol operations
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      export PG_PORT="${PG_PORT}"
      cd tests/go && go test -v -run Test_RaceExtendedProtocol ./extended
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_RaceExtendedProtocol"

  Scenario: Test disconnect handling
    When I run shell command:
      """
      export DATABASE_URL_DISCONNECT="postgresql://example_user_disconnect@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_Disconnect ./extended
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_Disconnect"

  Scenario: Test cancel query via TLS
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run TestCancelTLSQuery ./extended
      """
    Then the command should succeed
    And the command output should contain "PASS: TestCancelTLSQuery"

  Scenario: Test race stop handling
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      export PG_PORT="${PG_PORT}"
      cd tests/go && go test -v -run Test_RaceStop ./extended
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_RaceStop"
