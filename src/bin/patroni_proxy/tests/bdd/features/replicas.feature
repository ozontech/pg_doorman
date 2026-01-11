@patroni_proxy
Feature: Replica lag handling and connection management

  Scenario: Least connections balancing and lag-based disconnection with two replicas
    # Setup: two mock backend servers for replicas (each on different port, simulating different hosts)
    Given mock backend server 'replica1' for ping-pong protocol
    And mock backend server 'replica2' for ping-pong protocol
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
            "port": ${BACKEND_REPLICA1_PORT},
            "role": "replica",
            "state": "running",
            "api_url": "http://127.0.0.1:8008/patroni",
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
              host_port: ${BACKEND_REPLICA1_PORT}
              max_lag_in_bytes: 1000000
      """
    When wait for 2 seconds
    # Step 1: Open session to replica1 (only replica available)
    Given I open session to 'replicas' named 'session1'
    Then I execute ping on session 'session1' and receive pong
    And session 'session1' is connected to backend 'replica1'
    # Step 2: Update Patroni response to add replica2 with different port
    # Note: We update host_port in config to match replica2's port for new connections
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
            "port": ${BACKEND_REPLICA1_PORT},
            "role": "replica",
            "state": "running",
            "api_url": "http://127.0.0.1:8008/patroni",
            "timeline": 1,
            "lag": 0
          },
          {
            "name": "replica2",
            "host": "127.0.0.2",
            "port": ${BACKEND_REPLICA2_PORT},
            "role": "replica",
            "state": "running",
            "api_url": "http://127.0.0.2:8008/patroni",
            "timeline": 1,
            "lag": 0
          }
        ]
      }
      """
    And API /update_clusters is called
    And wait for 1 seconds
    # Step 3: Verify session1 is still alive (connection counter preserved after update)
    Then I execute ping on session 'session1' and receive pong
    And session 'session1' is connected to backend 'replica1'
    # Step 4: Set high lag on replica1 - session1 should be disconnected
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
            "port": ${BACKEND_REPLICA1_PORT},
            "role": "replica",
            "state": "running",
            "api_url": "http://127.0.0.1:8008/patroni",
            "timeline": 1,
            "lag": 999999999
          },
          {
            "name": "replica2",
            "host": "127.0.0.2",
            "port": ${BACKEND_REPLICA2_PORT},
            "role": "replica",
            "state": "running",
            "api_url": "http://127.0.0.2:8008/patroni",
            "timeline": 1,
            "lag": 0
          }
        ]
      }
      """
    And API /update_clusters is called
    And wait for 1 seconds
    # Step 5: Verify session1 is closed due to high lag
    Then session 'session1' is closed
