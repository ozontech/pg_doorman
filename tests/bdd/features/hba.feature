@hba
Feature: HBA authentication tests
  Test pg_doorman HBA trust authentication and deny rules

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      hostssl all             all             127.0.0.1/32            trust
      """
    And fixtures from "tests/fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      hostnossl all example_user_nopassword 127.0.0.1/32 reject
      hostssl all example_user_nopassword 127.0.0.1/32 trust
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"
      admin_username = "admin"
      admin_password = "admin"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "session"

      [pools.example_db.users.0]
      username = "example_user_nopassword"
      password = ""
      pool_size = 40
      """

  Scenario: Test HBA trust authentication
    When I run shell command:
      """
      export DATABASE_URL_TRUST="postgresql://example_user_nopassword@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=require"
      cd tests/go && go test -v -run Test_HbaTrust
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_HbaTrust"

  Scenario: Test HBA deny rules
    When I run shell command:
      """
      export DATABASE_URL_NOTRUST="postgresql://example_user_nopassword@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_HbaDeny
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_HbaDeny"
