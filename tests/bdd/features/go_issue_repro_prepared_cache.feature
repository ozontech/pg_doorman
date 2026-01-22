@go @prepared-cache @issue-repro
Feature: Go prepared statements cache size limit
  Test that prepared statements cache doesn't exceed the configured limit using Go pgx client

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 10

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  Scenario: Go cache size limit is respected after 100 different Parse requests
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run TestIssueReproPreparedCache ./prepared
      """
    Then the command should succeed
    And the command output should contain "PASS: TestIssueReproPreparedCache"
