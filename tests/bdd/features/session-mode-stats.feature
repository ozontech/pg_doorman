@pool @session-mode
Feature: Session mode statistics accuracy
  Regression test for query_time and xact_time percentiles in session mode.
  Without the fix, both metrics accumulate the entire session duration
  instead of individual query/transaction time — producing p99 values
  of 100+ seconds on long-lived sessions.

  Background:
    Given PostgreSQL started with options "-c max_connections=200" and pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """

  @session-query-time
  Scenario: query_time reflects individual queries, not session duration
    # 20 clients in session mode, each holding the connection for 5ms
    # (simulating a short query), repeated for 3 seconds. Individual
    # queries take ~5ms. If query_time accumulates, p99 will be in the
    # seconds range. The 500ms bound is generous enough for scheduling
    # noise but catches the 100-second accumulation bug.
    Given internal pool with size 10 and mode session
    When I run cascade load "session_query_check" with 20 clients for 3 seconds holding 5 ms
    Then cascade "session_query_check" reports zero errors
    And cascade "session_query_check" server query_p99 is below 500 ms
