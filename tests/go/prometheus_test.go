package doorman_test

import (
	"bufio"
	"context"
	"io"
	"net/http"
	"os"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v4"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// Test_Prometheus verifies selected Prometheus metrics are present and non-zero.
func Test_Prometheus(t *testing.T) {
	ctx := context.Background()

	// Create transactional activity so the exporter has data for transaction metrics
	session, errOpen := pgx.Connect(ctx, os.Getenv("DATABASE_URL_PROMETHEUS"))
	require.NoError(t, errOpen)
	// Execute many short transactions; add occasional small sleeps to make latencies non-zero
	for i := 0; i < 120; i++ {
		tx, err := session.Begin(ctx)
		require.NoError(t, err)
		// A quick statement
		_, err = tx.Exec(ctx, `select 1`)
		require.NoError(t, err)
		// Occasionally wait a bit to create measurable transaction durations
		if i%5 == 0 {
			_, err = tx.Exec(ctx, `select pg_sleep(0.02)`) // ~20ms
			require.NoError(t, err)
		}
		require.NoError(t, tx.Commit(ctx))
	}
	assert.NoError(t, session.Close(ctx))

	// Poll metrics endpoint with retries to allow exporter to update
	body := fetchMetricsWithRetry(t, "http://127.0.0.1:9127/metrics", 60, 250*time.Millisecond)

	// Common labels
	user := "example_user_prometheus"

	// Existing checks
	v1, ok := findMetricValue(body, "pg_doorman_connection_count", map[string]string{"type": "plain"})
	if !ok {
		t.Fatalf("metric pg_doorman_connection_count{type=\"plain\"} not found in exporter output")
	}
	assert.Greater(t, v1, 0.0, "pg_doorman_connection_count{type=\"plain\"} should be > 0")

	v2, ok := findMetricValue(body, "pg_doorman_pools_bytes", map[string]string{"user": user})
	if !ok {
		t.Fatalf("metric pg_doorman_pools_bytes{user=\"%s\"} not found", user)
	}
	assert.Greater(t, v2, 0.0, "pg_doorman_pools_bytes{user=\"%s\"} should be > 0", user)

	for _, p := range []string{"50", "90", "95", "99"} {
		v, ok := findMetricValue(body, "pg_doorman_pools_queries_percentile", map[string]string{"user": user, "percentile": p})
		if !ok {
			t.Fatalf("metric pg_doorman_pools_queries_percentile{user=\"%s\",percentile=\"%s\"} not found", user, p)
		}
		assert.Greater(t, v, 0.0, "pg_doorman_pools_queries_percentile{user=\"%s\",percentile=\"%s\"} should be > 0", user, p)
	}

	// New checks per issue description
	// 1) pg_doorman_pools_servers{database="example_db", user="example_user_prometheus"} > 0
	vs, ok := findMetricValue(body, "pg_doorman_pools_servers", map[string]string{"database": "example_db", "user": user, "status": "idle"})
	if !ok {
		t.Fatalf("metric pg_doorman_pools_servers{database=\"example_db\",user=\"%s\"} not found", user)
	}
	assert.Greater(t, vs, 0.0, "pg_doorman_pools_servers{database=\"example_db\",user=\"%s\"} should be > 0", user)

	// 2) pg_doorman_pools_transactions_count{user="example_user_prometheus"} ≥ 50
	vtc, ok := findMetricValue(body, "pg_doorman_pools_transactions_count", map[string]string{"user": user})
	if !ok {
		t.Fatalf("metric pg_doorman_pools_transactions_count{user=\"%s\"} not found", user)
	}
	assert.GreaterOrEqual(t, int(vtc), 50, "pg_doorman_pools_transactions_count{user=\"%s\"} should be >= 50", user)

	// 3) transactions percentiles 50, 90, 95, 99 > 0
	for _, p := range []string{"50", "90", "95", "99"} {
		v, ok := findMetricValue(body, "pg_doorman_pools_transactions_percentile", map[string]string{"user": user, "percentile": p})
		if !ok {
			t.Fatalf("metric pg_doorman_pools_transactions_percentile{user=\"%s\",percentile=\"%s\"} not found", user, p)
		}
		assert.Greater(t, v, 0.0, "pg_doorman_pools_transactions_percentile{user=\"%s\",percentile=\"%s\"} should be > 0", user, p)
	}

	// 4) total time > 0
	vtt, ok := findMetricValue(body, "pg_doorman_pools_transactions_total_time", map[string]string{"user": user})
	if !ok {
		t.Fatalf("metric pg_doorman_pools_transactions_total_time{user=\"%s\"} not found", user)
	}
	assert.Greater(t, vtt, 0.0, "pg_doorman_pools_transactions_total_time{user=\"%s\"} should be > 0", user)
}

// fetchMetricsWithRetry GETs the metrics endpoint with retries and returns the body as string.
func fetchMetricsWithRetry(t *testing.T, url string, attempts int, pause time.Duration) string {
	client := &http.Client{Timeout: 2 * time.Second}
	var lastErr error
	for i := 0; i < attempts; i++ {
		resp, err := client.Get(url)
		if err == nil && resp != nil && resp.StatusCode == 200 {
			b, err := io.ReadAll(resp.Body)
			_ = resp.Body.Close()
			if err == nil {
				return string(b)
			}
			lastErr = err
		} else if err != nil {
			lastErr = err
		} else if resp != nil {
			lastErr = io.ErrUnexpectedEOF
			_ = resp.Body.Close()
		}
		time.Sleep(pause)
	}
	t.Fatalf("failed to fetch metrics from %s: %v", url, lastErr)
	return ""
}

// findMetricValue scans Prometheus text exposition looking for a metric with the exact
// name and a superset of required label key/value pairs, then parses and returns its value.
func findMetricValue(metricsBody string, metricName string, required map[string]string) (float64, bool) {
	scanner := bufio.NewScanner(strings.NewReader(metricsBody))
	for scanner.Scan() {
		line := scanner.Text()
		// skip comments and empty lines
		if len(line) == 0 || strings.HasPrefix(line, "#") {
			continue
		}
		if !strings.HasPrefix(line, metricName) {
			continue
		}
		// Example line: metric{a="b",c="d"} 123
		labelsStart := strings.Index(line, "{")
		labelsEnd := strings.LastIndex(line, "}")
		var labelsStr string
		if labelsStart != -1 && labelsEnd != -1 && labelsEnd > labelsStart {
			labelsStr = line[labelsStart+1 : labelsEnd]
			// Ensure all required labels are present exactly as key="value"
			match := true
			for k, v := range required {
				needle := k + "=\"" + v + "\""
				if !strings.Contains(labelsStr, needle) {
					match = false
					break
				}
			}
			if !match {
				continue
			}
			// parse value: the token after the labels block
			valStr := strings.TrimSpace(line[labelsEnd+1:])
			// value may include an optional timestamp after the value; take first token
			if sp := strings.IndexAny(valStr, " \t"); sp != -1 {
				valStr = valStr[:sp]
			}
			if v, err := strconv.ParseFloat(valStr, 64); err == nil {
				return v, true
			}
		}
	}
	return 0, false
}
