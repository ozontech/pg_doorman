@patroni_fallback
Feature: Patroni-assisted fallback

  Scenario: Query succeeds via Patroni-assisted fallback when local PG is down
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
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      fallback_cooldown = "5s"
      fallback_connect_timeout = "3s"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"

  Scenario: Auth error does not trigger Patroni API call
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
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      fallback_cooldown = "5s"
      fallback_connect_timeout = "3s"
      patroni_api_timeout = "3s"

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
      patroni_api_urls = ["http://127.0.0.1:59998"]
      fallback_cooldown = "5s"
      fallback_connect_timeout = "3s"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "" fails

  Scenario: Fallback succeeds via second Patroni URL when first is unreachable
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
      patroni_api_urls = ["http://127.0.0.1:59997", "http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      fallback_cooldown = "5s"
      fallback_connect_timeout = "3s"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"

  Scenario: Connection fails when all cluster members are unreachable via TCP
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
            "port": 59996,
            "role": "sync_standby",
            "state": "streaming",
            "timeline": 1
          },
          {
            "name": "node2",
            "host": "127.0.0.1",
            "port": 59995,
            "role": "replica",
            "state": "streaming",
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
      server_port = 59999
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      fallback_cooldown = "5s"
      fallback_connect_timeout = "3s"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "" fails

  Scenario: Doorman uses updated member list after mock Patroni response changes
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
            "port": 59998,
            "role": "sync_standby",
            "state": "streaming",
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
      server_port = 59999
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      fallback_cooldown = "2s"
      fallback_connect_timeout = "3s"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "" fails
    When mock Patroni server 'patroni1' response is updated to:
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
            "timeline": 1
          }
        ]
      }
      """
    And we sleep for 3000 milliseconds
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"

  Scenario: Patroni member with lag "unknown" does not break cluster parsing
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
            "name": "node-stopped",
            "host": "127.0.0.1",
            "port": 59999,
            "role": "replica",
            "state": "stopped",
            "timeline": 1,
            "lag": "unknown",
            "receive_lsn": "unknown",
            "replay_lsn": "unknown"
          },
          {
            "name": "node-healthy",
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
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      fallback_cooldown = "5s"
      fallback_connect_timeout = "3s"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"

  Scenario: URL with trailing /cluster works correctly
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
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}/cluster"]
      fallback_cooldown = "5s"
      fallback_connect_timeout = "3s"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"

  Scenario: Global patroni_api_urls inherited by pool
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
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      fallback_cooldown = "5s"
      fallback_connect_timeout = "3s"
      patroni_api_timeout = "3s"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = 59999

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"

  Scenario: Patroni member with noloadbalance tag and string lag values
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
            "name": "node-nobalance",
            "host": "127.0.0.1",
            "port": 59999,
            "role": "replica",
            "state": "stopped",
            "timeline": 1,
            "tags": {"noloadbalance": true, "noloadagent": "safe_shutdown"},
            "lag": "unknown",
            "receive_lag": "unknown",
            "replay_lag": "unknown",
            "lsn": "unknown"
          },
          {
            "name": "node-leader",
            "host": "127.0.0.1",
            "port": 59998,
            "role": "leader",
            "state": "running",
            "timeline": 1
          },
          {
            "name": "node-healthy",
            "host": "127.0.0.1",
            "port": ${PG_PORT},
            "role": "replica",
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
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      fallback_cooldown = "5s"
      fallback_connect_timeout = "3s"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"
