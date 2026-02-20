@generate
Feature: Generate configuration command
  pg_doorman should generate valid configuration files in both TOML and YAML formats
  that can be used to start pg_doorman and establish connections.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  Scenario: Generate TOML config and verify connectivity
    When we generate pg_doorman config with args "--host 127.0.0.1 --port ${PG_PORT} -u example_user_1 --password test -d example_db" to "toml" format
    Then the command should succeed
    Given pg_doorman started with generated config
    When I run shell command "psql 'host=127.0.0.1 port=${DOORMAN_PORT} user=example_user_1 dbname=example_db' -c 'SELECT 1'"
    Then the command should succeed

  Scenario: Generate YAML config and verify connectivity
    When we generate pg_doorman config with args "--host 127.0.0.1 --port ${PG_PORT} -u example_user_1 --password test -d example_db" to "yaml" format
    Then the command should succeed
    Given pg_doorman started with generated config
    When I run shell command "psql 'host=127.0.0.1 port=${DOORMAN_PORT} user=example_user_1 dbname=example_db' -c 'SELECT 1'"
    Then the command should succeed
