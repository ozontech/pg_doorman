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


  Scenario: Fallback iterates past dead sync_standby to live replica
    # B''. Best candidate (sync_standby) refuses TCP; the next priority
    # (replica) is alive — fallback must reach it instead of giving up.
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And mock Patroni server 'p1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {"name": "sync-dead",   "host": "127.0.0.1", "port": 59995,         "role": "sync_standby", "state": "streaming", "timeline": 1, "lag": 0},
          {"name": "replica-live","host": "127.0.0.1", "port": ${PG_PORT},    "role": "replica",      "state": "streaming", "timeline": 1, "lag": 0}
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
      connect_timeout = "2s"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = 59999
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_P1_PORT}"]
      fallback_cooldown = "5s"
      fallback_connect_timeout = "1s"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"

  Scenario: Cooldown lets the second client request reach the live candidate without retrying the dead one
    # C. After the first request fails on the dead sync_standby and falls
    # through to the live replica, the second request must also succeed —
    # cooldown should not break repeat queries.
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And mock Patroni server 'p1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {"name": "sync-dead",   "host": "127.0.0.1", "port": 59995,         "role": "sync_standby", "state": "streaming", "timeline": 1, "lag": 0},
          {"name": "replica-live","host": "127.0.0.1", "port": ${PG_PORT},    "role": "replica",      "state": "streaming", "timeline": 1, "lag": 0}
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
      connect_timeout = "2s"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = 59999
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_P1_PORT}"]
      fallback_cooldown = "30s"
      fallback_connect_timeout = "5s"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"
    When we sleep for 500 milliseconds
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"

  Scenario: All fallback candidates hang on startup — exhaustion error names reasons
    # E + G. Two candidates whose TCP listener accepts but never replies to
    # StartupMessage exercise startup_with_timeout (Timeout reason) and the
    # exhaustion summary "all fallback candidates rejected (...)" returned
    # to the client.
    Given we start hung TCP listener as 'sync'
    And we start hung TCP listener as 'replica'
    And PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And mock Patroni server 'p1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {"name": "sync-hung",   "host": "127.0.0.1", "port": ${HUNG_SYNC_PORT},   "role": "sync_standby", "state": "streaming", "timeline": 1, "lag": 0},
          {"name": "replica-hung","host": "127.0.0.1", "port": ${HUNG_REPLICA_PORT},"role": "replica",      "state": "streaming", "timeline": 1, "lag": 0}
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
      connect_timeout = "1s"
      query_wait_timeout = "10s"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = 59999
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_P1_PORT}"]
      fallback_cooldown = "5s"
      fallback_connect_timeout = "300ms"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "" fails with error containing "all fallback candidates rejected"

  Scenario: Fallback total deadline aborts when query_wait_timeout elapses
    # A'. With local PG dead and a single hung Patroni member, query_wait_timeout
    # caps how long the client waits before getting an error — without the
    # outer deadline pg_doorman would loop through cooldown→discovery
    # indefinitely.
    Given we start hung TCP listener as 'only'
    And PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And mock Patroni server 'p1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {"name": "only-hung", "host": "127.0.0.1", "port": ${HUNG_ONLY_PORT}, "role": "sync_standby", "state": "streaming", "timeline": 1, "lag": 0}
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
      connect_timeout = "500ms"
      query_wait_timeout = "2s"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = 59999
      patroni_api_urls = ["http://127.0.0.1:${PATRONI_P1_PORT}"]
      fallback_cooldown = "30s"
      fallback_connect_timeout = "10s"
      patroni_api_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "" fails
