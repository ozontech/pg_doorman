@rust @pool-size-show-pools
Feature: pool_size column in SHOW POOLS
  Verify that SHOW POOLS exposes the configured pool_size for both
  static (config-defined) and dynamic (auth_query passthrough) pools.

  @pool-size-static
  Scenario: Static pool shows correct pool_size in SHOW POOLS
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: "admin"
        admin_password: "admin"
        pg_hba:
          content: "host all all 127.0.0.1/32 trust"
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          users:
            - username: "example_user_1"
              password: ""
              pool_size: 7
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1"
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "pool_size" should be between 7 and 7

  @pool-size-dynamic
  Scenario: Dynamic pool shows correct pool_size in SHOW POOLS
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_passthrough_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 md5
      """
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
        admin_username: "admin"
        admin_password: "admin"
        tls_private_key: "${DOORMAN_SSL_KEY}"
        tls_certificate: "${DOORMAN_SSL_CERT}"
        pg_hba:
          path: "${DOORMAN_HBA_FILE}"
      pools:
        postgres:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: "transaction"
          auth_query:
            query: "SELECT username, password FROM auth_users WHERE username = $1"
            user: "postgres"
            password: ""
            pool_size: 1
            default_pool_size: 3
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    Then psql query "SELECT current_user" via pg_doorman as user "pt_md5_user" to database "postgres" with password "md5_pass" returns "pt_md5_user"
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "pool_size" for row with "user" = "pt_md5_user" should be between 3 and 3
