@patroni_failover
Feature: Patroni failover discovery

  Scenario: Query succeeds via Patroni failover when local PG is down
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And mock Patroni server 'patroni1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {
            "name": "node1",
            "host": "127.0.0.1",
            "port": 59999,
            "role": "leader",
            "state": "stopped",
            "timeline": 1
          },
          {
            "name": "node2",
            "host": "127.0.0.1",
            "port": ${PG_PORT},
            "role": "sync_standby",
            "state": "streaming",
            "timeline": 1,
            "lag": 0
          }
        ]
      }
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      connect_timeout = "5s"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = 59999
      patroni_discovery_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      failover_blacklist_duration = "5s"
      failover_connect_timeout = "3s"
      failover_discovery_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"

  Scenario: Auth error does not trigger Patroni discovery
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
      """
    And fixtures from "tests/fixture.sql" applied
    And mock Patroni server 'patroni1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {
            "name": "node1",
            "host": "127.0.0.1",
            "port": ${PG_PORT},
            "role": "leader",
            "state": "running",
            "timeline": 1
          }
        ]
      }
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      connect_timeout = "5s"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      patroni_discovery_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      failover_blacklist_duration = "5s"
      failover_connect_timeout = "3s"
      failover_discovery_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      server_username = "example_user_1"
      server_password = "wrong_password"
      pool_size = 5
      """
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "" fails

  Scenario: Connection fails when all Patroni URLs are unreachable
    Given PostgreSQL started with pg_hba.conf:
      """
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
      connect_timeout = "5s"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = 59999
      patroni_discovery_urls = ["http://127.0.0.1:59998"]
      failover_blacklist_duration = "5s"
      failover_connect_timeout = "3s"
      failover_discovery_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "" fails
