@go @check_query
Feature: Check query functionality tests
  Test pg_doorman check query functionality

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 md5
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      connect_timeout = 5000
      admin_username = "admin"
      admin_password = "admin"
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.example_db.users.0]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 40
      """

  Scenario: Test check query functionality
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run "^TestCheckQuery$"
      """
    Then the command should succeed
