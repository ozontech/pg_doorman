Feature: Connection through pg_doorman

  Scenario: Successful connection to PostgreSQL through pg_doorman
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      connect_timeout = 5000
      admin_username = "admin"
      admin_password = "admin"

      [pools.postgres]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [pools.postgres.users.0]
      username = "postgres"
      password = "md53175bce1d3201d16594cebf9d7eb3f9d"
      pool_size = 10
      """
    Then psql connection to pg_doorman as user "postgres" to database "postgres" with password "postgres" succeeds
