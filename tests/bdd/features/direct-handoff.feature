@pool @direct-handoff
Feature: Direct handoff — return_object delivers via oneshot channel
  Regression harness for the oneshot-channel handoff path in
  return_object. When a connection is returned and the waiters queue
  is non-empty, the connection is delivered directly to the oldest
  waiter via a oneshot channel — no idle VecDeque push, no semaphore
  add_permits, no FIFO position loss. These scenarios pin the
  observable properties of that path: zero errors, bounded pool size,
  bounded tail latency, bounded connection churn, and fair iteration
  spread across clients.

  Background:
    Given PostgreSQL started with options "-c max_connections=200" and pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """

  @handoff-saturated
  Scenario: Saturated pool serves all clients through handoff without growing
    # 50 clients on 5 slots, hold=10ms. Steady-state throughput is
    # ~500 acquires/sec per slot. Every return finds a waiter and hands
    # off directly — the idle VecDeque stays empty, pool size stays at 5.
    # creates_started <= 10 allows warm-up wobble; create_fallback <= 3
    # confirms almost all acquires were served by handoff, not by the
    # fallback create path.
    Given internal pool with size 5 and mode transaction
    When I run cascade load "handoff_saturated" with 50 clients for 5 seconds holding 10 ms
    Then cascade "handoff_saturated" reports zero errors
    And cascade "handoff_saturated" pool size is at most 5
    And cascade "handoff_saturated" pool size is at least 5
    And cascade "handoff_saturated" creates_started is at most 10
    And cascade "handoff_saturated" create_fallback is at most 5

  @handoff-fifo
  Scenario: FIFO ordering keeps iteration spread and tail latency bounded
    # 200 clients on 5 slots. The waiters queue is FIFO: the oldest
    # waiter gets the next return. Without FIFO, a few clients would
    # starve (low iteration count) while peers race ahead, and the max
    # latency would blow up relative to p50. The iteration spread gate
    # catches client-level starvation; the max/p50 gate catches single
    # stuck acquires.
    Given internal pool with size 5 and mode transaction
    When I run cascade load "handoff_fifo" with 200 clients for 5 seconds holding 10 ms
    Then cascade "handoff_fifo" reports zero errors
    And cascade "handoff_fifo" iteration spread is bounded by 100x median
    And cascade "handoff_fifo" max latency is bounded by 20x p50

  @handoff-timeout-skip
  Scenario: Timed-out waiters are skipped without errors or pool growth
    # 100 clients on 2 slots, hold=50ms. Many waiters will time out
    # before receiving a handoff. return_object must skip their dead
    # senders (oneshot receiver dropped) and deliver to the next live
    # waiter. The pool must not grow beyond 2 and must not leak
    # connections.
    Given internal pool with size 2 and mode transaction
    When I run cascade load "handoff_timeout_skip" with 100 clients for 5 seconds holding 50 ms
    Then cascade "handoff_timeout_skip" reports zero errors
    And cascade "handoff_timeout_skip" pool size is at most 2
    And cascade "handoff_timeout_skip" pool size is at least 2
    And cascade "handoff_timeout_skip" creates_started is at most 10

  @handoff-all-expired
  Scenario: All waiters expired — connection lands in idle queue without errors
    # 200 clients on 2 slots, hold=100ms. Extreme contention: most
    # waiters expire before any connection returns. When return_object
    # exhausts the waiters queue (every send fails), it must fall back
    # to the idle VecDeque push path. The pool must survive without
    # errors and keep at least 2 connections alive.
    Given internal pool with size 2 and mode transaction
    When I run cascade load "handoff_all_expired" with 200 clients for 3 seconds holding 100 ms
    Then cascade "handoff_all_expired" reports zero errors
    And cascade "handoff_all_expired" pool size is at least 2

  @handoff-burst-gate
  Scenario: Burst gate funnels through handoff with bounded tail latency
    # 200 clients on 10 slots, hold=20ms. The burst gate limits
    # concurrent creates; excess callers register as waiters and receive
    # connections via handoff. p99 < 600ms confirms the handoff path
    # does not add significant latency on top of the wait budget.
    Given internal pool with size 10 and mode transaction
    When I run cascade load "handoff_burst" with 200 clients for 5 seconds holding 20 ms
    Then cascade "handoff_burst" reports zero errors
    And cascade "handoff_burst" pool size is at most 10
    And cascade "handoff_burst" pool size is at least 10
    And cascade "handoff_burst" creates_started is at most 20
    And cascade "handoff_burst" p99 latency is below 600 ms

  @handoff-high-contention
  Scenario: High contention — 500 clients on 10 slots with bounded p99
    # Stress test: 500 concurrent clients competing for 10 slots.
    # The handoff path must keep p99 below 800ms and the pool must
    # not grow. Iteration spread confirms no client starvation under
    # extreme contention on the slots mutex.
    Given internal pool with size 10 and mode transaction
    When I run cascade load "handoff_stress" with 500 clients for 5 seconds holding 10 ms
    Then cascade "handoff_stress" reports zero errors
    And cascade "handoff_stress" pool size is at most 10
    And cascade "handoff_stress" pool size is at least 10
    And cascade "handoff_stress" p99 latency is below 800 ms
    And cascade "handoff_stress" iteration spread is bounded by 100x median
