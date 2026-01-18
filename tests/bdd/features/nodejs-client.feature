@nodejs
Feature: Node.js client tests
  Test pg_doorman with Node.js PostgreSQL client (pg)

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      connect_timeout = 5000
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
      pool_size = 10
      """

  @nodejs-basic
  Scenario: Run Node.js basic client tests
    When I run shell command:
      """
      cd tests/nodejs && \
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db" && \
      npm install pg && \
      node ./test_simple.js
      """
    Then the command should succeed

  @nodejs-prepared
  Scenario: Run Node.js prepared statements tests
    When I run shell command:
      """
      cd tests/nodejs && \
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db" && \
      npm install pg && \
      node ./test_prepared.js
      """
    Then the command should succeed

  @nodejs-errors
  Scenario: Run Node.js error handling tests
    When I run shell command:
      """
      cd tests/nodejs && \
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db" && \
      npm install pg && \
      node ./test_errors.js
      """
    Then the command should succeed

  @nodejs-transactions
  Scenario: Run Node.js transaction tests
    When I run shell command:
      """
      cd tests/nodejs && \
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db" && \
      npm install pg && \
      node ./test_transactions.js
      """
    Then the command should succeed

  @nodejs-edge-cases
  Scenario: Run Node.js edge case tests
    When I run shell command:
      """
      cd tests/nodejs && \
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db" && \
      npm install pg && \
      node ./test_edge_cases.js
      """
    Then the command should succeed
