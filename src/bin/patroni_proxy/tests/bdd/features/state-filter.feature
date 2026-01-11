@patroni_proxy
Feature: Member state filtering

  Scenario: Only members with state 'running' are used as backends
    # Setup: mock backend server for master
    Given mock backend server 'pg_master' for ping-pong protocol
    And mock Patroni server 'node1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {
            "name": "node1",
            "host": "127.0.0.1",
            "port": ${BACKEND_PG_MASTER_PORT},
            "role": "leader",
            "state": "running",
            "api_url": "http://127.0.0.1:8008/patroni",
            "timeline": 1
          },
          {
            "name": "node2",
            "host": "127.0.0.1",
            "port": 15432,
            "role": "replica",
            "state": "starting",
            "api_url": "http://127.0.0.1:8009/patroni",
            "timeline": 1,
            "lag": 0
          },
          {
            "name": "node3",
            "host": "127.0.0.1",
            "port": 15433,
            "role": "replica",
            "state": "stopped",
            "api_url": "http://127.0.0.1:8010/patroni",
            "timeline": 1,
            "lag": 0
          }
        ]
      }
      """
    And API listen address is allocated
    And proxy port 'master' is allocated
    And proxy port 'replicas' is allocated
    And patroni_proxy started with config:
      """
      cluster_update_interval: 1
      listen_address: "${API_ADDR}"

      clusters:
        test_cluster:
          hosts:
            - "http://127.0.0.1:${PATRONI_NODE1_PORT}"
          
          ports:
            master:
              listen: "${PROXY_MASTER_ADDR}"
              roles: ["leader"]
              host_port: ${BACKEND_PG_MASTER_PORT}
            
            replicas:
              listen: "${PROXY_REPLICAS_ADDR}"
              roles: ["sync", "async"]
              host_port: 15432
      """
    When wait for 2 seconds
    # Master with state 'running' should be accessible
    Then TCP connection to proxy port 'master' succeeds
    Given I open session to 'master' named 'master_session'
    Then I execute ping on session 'master_session' and receive pong
    # Replicas with state 'starting' and 'stopped' should NOT be used
    # Connection to replicas port accepts but immediately closes (no backends)
    Given I open session to 'replicas' named 'replica_session'
    Then session 'replica_session' is closed

  Scenario: Member changes state from 'starting' to 'running' becomes available
    Given mock backend server 'pg_replica' for ping-pong protocol
    And mock Patroni server 'node1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {
            "name": "node1",
            "host": "127.0.0.1",
            "port": 5432,
            "role": "leader",
            "state": "running",
            "api_url": "http://127.0.0.1:8008/patroni",
            "timeline": 1
          },
          {
            "name": "replica1",
            "host": "127.0.0.1",
            "port": ${BACKEND_PG_REPLICA_PORT},
            "role": "replica",
            "state": "starting",
            "api_url": "http://127.0.0.1:8009/patroni",
            "timeline": 1,
            "lag": 0
          }
        ]
      }
      """
    And API listen address is allocated
    And proxy port 'replicas' is allocated
    And patroni_proxy started with config:
      """
      cluster_update_interval: 1
      listen_address: "${API_ADDR}"

      clusters:
        test_cluster:
          hosts:
            - "http://127.0.0.1:${PATRONI_NODE1_PORT}"
          
          ports:
            replicas:
              listen: "${PROXY_REPLICAS_ADDR}"
              roles: ["sync", "async"]
              host_port: ${BACKEND_PG_REPLICA_PORT}
      """
    When wait for 2 seconds
    # Replica with state 'starting' should NOT be available
    # Connection accepts but immediately closes (no backends)
    Given I open session to 'replicas' named 'starting_session'
    Then session 'starting_session' is closed
    # Update replica state to 'running'
    When mock Patroni server 'node1' response is updated to:
      """
      {
        "scope": "test_cluster",
        "members": [
          {
            "name": "node1",
            "host": "127.0.0.1",
            "port": 5432,
            "role": "leader",
            "state": "running",
            "api_url": "http://127.0.0.1:8008/patroni",
            "timeline": 1
          },
          {
            "name": "replica1",
            "host": "127.0.0.1",
            "port": ${BACKEND_PG_REPLICA_PORT},
            "role": "replica",
            "state": "running",
            "api_url": "http://127.0.0.1:8009/patroni",
            "timeline": 1,
            "lag": 0
          }
        ]
      }
      """
    And API /update_clusters is called
    And wait for 1 seconds
    # Now replica should be available
    Then TCP connection to proxy port 'replicas' succeeds
    Given I open session to 'replicas' named 'replica_session'
    Then I execute ping on session 'replica_session' and receive pong
