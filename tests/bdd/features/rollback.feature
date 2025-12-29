@rollback
Feature: Rollback functionality tests
  Test pg_doorman automatic rollback functionality

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
      host all all 127.0.0.1/32 md5
      """
    And self-signed SSL certificates are generated
    And pg_doorman started with config:
      """
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

      [pools.example_db.users.0]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 40

      [pools.example_db.users.1]
      username = "example_user_rollback"
      password = "md58b67c8b2b2370f3b5ee2416999588830"
      pool_size = 40
      pool_mode = "session"
      """

  Scenario: Test automatic rollback functionality
    When I run shell command:
      """
      export DATABASE_URL_ROLLBACK="postgresql://example_user_rollback:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_Rollback$ ./rollback
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_Rollback"

  Scenario: Test savepoint rollback functionality
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      export DATABASE_URL_ROLLBACK="postgresql://example_user_rollback:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_RollbackSavePoint ./rollback
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_RollbackSavePoint"

