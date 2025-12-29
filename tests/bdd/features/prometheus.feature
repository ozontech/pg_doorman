@prometheus
Feature: Prometheus metrics tests
  Test pg_doorman Prometheus metrics endpoint

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
      host all example_user_prometheus 127.0.0.1/32 trust
      """
    And self-signed SSL certificates are generated
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
      admin_username = "admin"
      admin_password = "admin"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.example_db.users.0]
      username = "example_user_prometheus"
      password = ""
      pool_size = 40
      """

  Scenario: Test Prometheus metrics endpoint
    When I run shell command:
      """
      export DATABASE_URL_PROMETHEUS="postgresql://example_user_prometheus@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_Prometheus ./prometheus
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_Prometheus"
