@pool @pool-pressure
Feature: Pool under sustained client pressure
  Regression harness for Phase 4 anticipation behavior and pool sizing
  decisions. Each scenario pins a specific property of `Pool::timeout_get`
  under a realistic cascade — many more clients than the pool can hold,
  each client briefly holding a connection. The measured quantities are
  tail latency, the number of new backend connections created, Phase 4
  fallback counts, and the final pool size. Any change to the Phase 4
  loop, recycle-skip gate, or retain-skip gate is expected to hold these
  properties; a regression in any of them fails the scenario.

  Background:
    Given PostgreSQL started with options "-c max_connections=200" and pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """

  @cascade-baseline
  Scenario: 300 clients sharing 10 slots complete without errors and stay on the hot recycle path
    # Steady-state test: 300 clients × hold=10ms on pool_size=10. Each client
    # waits `clients/pool_size * hold_ms ≈ 300ms` on the semaphore, then the
    # Phase 1 hot-recycle path serves it immediately because a peer has just
    # returned. Phase 4 anticipation should stay idle — if a regression in the
    # semaphore wake ordering or the recycle path starts pushing callers into
    # anticipation, create_fallback or antic_timeout will grow and fail the
    # scenario. The latency bound is a generous 600 ms (observed p99 ~330 ms)
    # so routine scheduling noise does not cause false positives.
    Given internal pool with size 10 and mode transaction
    When I run cascade load "cascade_c300_pool10" with 300 clients for 5 seconds holding 10 ms
    Then cascade "cascade_c300_pool10" reports zero errors
    And cascade "cascade_c300_pool10" p99 latency is below 600 ms
    And cascade "cascade_c300_pool10" creates_started is at most 20
    And cascade "cascade_c300_pool10" create_fallback is at most 5
    And cascade "cascade_c300_pool10" pool size is at most 10
    And cascade "cascade_c300_pool10" pool size is at least 10

  @cascade-recycle-skip
  Scenario: server_lifetime expiration is suppressed while the pool is under sustained pressure
    # Regression harness for the recycle-skip commit: when the semaphore is
    # fully drained, `recycle()` must not close an aged connection on the way
    # back into the pool. Otherwise a short `server_lifetime` on a permanently
    # busy pool rotates every connection on every return, turning a working
    # pool of ten into a cascade of connect() calls.
    #
    # Setup: 10 slots, server_lifetime=200ms, 100 clients × 10 seconds ×
    # hold=10ms. The load is continuous and every permit is in flight, so the
    # pool is under_pressure() for the entire run. Each server connection
    # lives far past its 200ms budget but must NOT be closed by recycle.
    #
    # Without recycle skip: every return would trip the age check, close the
    # connection and force a new connect. 10 slots × (10s / 200ms) = ~500
    # forced rotations. With recycle skip: the warm-up grows the pool to 10
    # and then nothing else creates — the bound is a tight upper limit on
    # churn, so a regression in the gate is caught immediately.
    Given internal pool with size 10, server_lifetime 200 ms, idle_timeout 0 ms
    When I run cascade load "recycle_skip_c100_pool10" with 100 clients for 10 seconds holding 10 ms
    Then cascade "recycle_skip_c100_pool10" reports zero errors
    And cascade "recycle_skip_c100_pool10" creates_started is at most 20
    And cascade "recycle_skip_c100_pool10" pool size is at most 10
    And cascade "recycle_skip_c100_pool10" pool size is at least 10

  @cascade-recycle-resume
  Scenario: lifetime expiration resumes once client pressure clears
    # Companion to @cascade-recycle-skip: the skip gate must be a brake, not
    # a latch. Once the semaphore has spare permits again, recycle() must
    # enforce server_lifetime on the very next acquire of an aged connection.
    # A regression that makes recycle-skip permanent would freeze the pool
    # with stale backends forever.
    #
    # Phase 1 fills the pool under pressure (same shape as the recycle-skip
    # scenario). The sleep lets every connection age past server_lifetime.
    # Phase 2 runs a single client so the semaphore is NOT under pressure —
    # under_pressure() returns false, recycle() checks the age and closes
    # each aged connection the client touches, creates a fresh one, and the
    # counter ticks up.
    #
    # The assertion is deliberately loose (at least one rotation in the
    # quiet phase): LIFO queue mode means the single client reuses the
    # same slot repeatedly, so only the actively-touched connection is
    # rotated within the short window. The test still fails if the gate
    # is broken and zero rotations happen.
    Given internal pool with size 10, server_lifetime 200 ms, idle_timeout 0 ms
    When I run cascade load "recycle_resume_burst" with 100 clients for 2 seconds holding 5 ms
    And we sleep for 500 milliseconds
    And I run cascade load "recycle_resume_quiet" with 1 clients for 2 seconds holding 5 ms
    Then cascade "recycle_resume_burst" reports zero errors
    And cascade "recycle_resume_burst" creates_started is at most 20
    And cascade "recycle_resume_burst" iteration spread is bounded by 100x median
    And cascade "recycle_resume_quiet" reports zero errors
    And cascade "recycle_resume_quiet" creates_started is at least 1
