@patroni_proxy
Feature: Basic patroni_proxy functionality

  Scenario: Proxy with mock Patroni cluster
    Given mock Patroni server 'node1' with response:
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
          },
          {
            "name": "node3",
            "host": "127.0.0.1",
            "port": 5434,
            "role": "replica",
            "state": "running",
            "api_url": "http://127.0.0.1:8008/patroni",
            "timeline": 1,
            "lag": 1024
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
              host_port: 5432
            
            replicas:
              listen: "${PROXY_REPLICAS_ADDR}"
              roles: ["sync", "async"]
              host_port: 5432
      """
    When wait for 3 seconds
    Then TCP connection to proxy port 'master' succeeds
    And TCP connection to proxy port 'replicas' succeeds
