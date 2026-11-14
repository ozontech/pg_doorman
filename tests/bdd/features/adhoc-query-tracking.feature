@rust @rust-4 @adhoc-tracking
Feature: Approximate novelty of Parse messages
  Tracking observes first Parse hashes and follows RELOAD on retained clients.

  Background:
    Given PostgreSQL for adhoc tracking started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And adhoc metrics listener uses a free port

  Scenario: Live toggles preserve backend connections and cumulative metrics
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: "admin"
        admin_password: "admin"
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        prepared_statements: true
        adhoc_query_tracking: false
      web:
        enabled: true
        host: "127.0.0.1"
        port: ${ADHOC_METRICS_PORT}
      pools:
        postgres:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: "transaction"
          users:
            - username: "postgres"
              password: ""
              pool_size: 1
      """
    Then adhoc tracking follows Parse novelty and live reload without reconnecting

  Scenario: Prepared-disabled passthrough tracks SQL without statement names
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: "admin"
        admin_password: "admin"
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        prepared_statements: false
        adhoc_query_tracking: true
      web:
        enabled: true
        host: "127.0.0.1"
        port: ${ADHOC_METRICS_PORT}
      pools:
        postgres:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: "transaction"
          users:
            - username: "postgres"
              password: ""
              pool_size: 1
      """
    Then adhoc tracking hashes raw Parse SQL when prepared statements are disabled
