@patroni_proxy
Feature: Configuration reload for patroni_proxy

  Scenario: Reload configuration and verify new port is accessible
    Given mock backend server 'pg_master' for ping-pong protocol
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
            "name": "node2",
            "host": "127.0.0.1",
            "port": 5433,
            "role": "sync_standby",
            "state": "running",
            "api_url": "http://127.0.0.1:8008/patroni",
            "timeline": 1,
            "lag": 0
          }
        ]
      }
      """
    And API listen address is allocated
    And proxy port 'master' is allocated
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
      """
    When wait for 1 seconds
    Then TCP connection to proxy port 'master' succeeds
    Given I open session to 'master' named 'master'
    Then I execute ping on session 'master' and receive pong
    Given proxy port 'replicas' is allocated
    When patroni_proxy config is modified:
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
              host_port: ${BACKEND_PG_MASTER_PORT}
      """
    And patroni_proxy receives SIGHUP signal
    And wait for 2 seconds
    Then I execute ping on session 'master' and receive pong
    And TCP connection to proxy port 'replicas' succeeds
