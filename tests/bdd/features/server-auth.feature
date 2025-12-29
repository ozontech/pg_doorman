@go @server-auth
Feature: Server authentication tests
  Test pg_doorman server authentication (MD5, SCRAM, JWT)

  Scenario: Test MD5 server authentication
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
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
      server_username = "example_user_1"
      server_password = "test"
      pool_size = 10

      [pools.example_db.users.1]
      username = "example_user_bad"
      password = ""
      server_username = "example_user_1"
      server_password = "wrong_password"
      pool_size = 10
      """
    When I run shell command:
      """
      export DATABASE_URL_MD5_AUTH_OK="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      export DATABASE_URL_MD5_AUTH_BAD="postgresql://example_user_bad@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run "TestServerAuthMD5" ./server-auth
      """
    Then the command should succeed
    And the command output should contain "PASS: TestServerAuthMD5OK"
    And the command output should contain "PASS: TestServerAuthMD5BAD"

  Scenario: Test SCRAM server authentication
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            scram-sha-256
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
      username = "example_user_scram"
      password = "SCRAM-SHA-256$4096:p2j/1lMdQF6r1dD9I9f7PQ==$H3xt5yh7lwSq9zUPYwHovRu3FyUCCXchG/skydJRa9o=:5xU6Wj/GNg3UnN2uQIx3ezx7uZyzGeM5NrvSJRIxnlw="
      server_username = "example_user_2"
      server_password = "test"
      pool_size = 10

      [pools.example_db.users.1]
      username = "example_user_scram_bad"
      password = ""
      server_username = "example_user_2"
      server_password = "wrong_password"
      pool_size = 10
      """
    When I run shell command:
      """
      export DATABASE_URL_SCRAM_AUTH_OK="postgresql://example_user_scram:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      export DATABASE_URL_SCRAM_AUTH_BAD="postgresql://example_user_scram_bad@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run "TestServerAuthSCRAM" ./server-auth
      """
    Then the command should succeed
    And the command output should contain "PASS: TestServerAuthSCRAMOK"
    And the command output should contain "PASS: TestServerAuthSCRAMBAD"
