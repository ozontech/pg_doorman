@python
Feature: Python client tests
  Test pg_doorman with Python PostgreSQL clients (psycopg2, asyncpg)

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
      host all all 127.0.0.1/32 trust
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
      pool_size = 10
      """

  Scenario: Run Python async tests
    When I run shell command:
      """
      cd tests/python && \
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db" && \
      python3 ./test_async.py
      """
    Then the command should succeed

  Scenario: Run Python psycopg2 tests
    When I run shell command:
      """
      cd tests/python && \
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db" && \
      python3 ./test_psycopg2.py
      """
    Then the command should succeed

  Scenario: Run Python session cursors tests
    When I run shell command:
      """
      cd tests/python && \
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db" && \
      python3 ./test_session_cursors.py
      """
    Then the command should succeed

  Scenario: Run Python cancel query tests
    When I run shell command:
      """
      cd tests/python && \
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db" && \
      pytest ./test_cancel_query.py
      """
    Then the command should succeed
