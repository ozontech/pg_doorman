package doorman_test

import (
	"context"
	"os"
	"testing"
	"time"

	"github.com/jackc/pgx/v4/pgxpool"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// The `;` check query must not be accounted as a transaction; ensure the
// pg_doorman_pools_transactions_count metric remains unchanged after executing it.
func TestCheckQueryDoesNotAffectTransactionsCount(t *testing.T) {
	ctx := context.Background()
	user := "example_user_1"

	// Get baseline metrics value
	beforeBody := fetchMetricsWithRetry(t, "http://127.0.0.1:9127/metrics", 20, 250*time.Millisecond)
	before, ok := findMetricValue(beforeBody, "pg_doorman_pools_transactions_count", map[string]string{"user": user})
	if !ok {
		t.Fatalf("metric pg_doorman_pools_transactions_count{user=\"%s\"} not found", user)
	}

	// Execute the check query
	db, err := pgxpool.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer db.Close()
	_, err = db.Exec(ctx, ";")
	require.NoError(t, err)

	// Fetch metrics again and ensure the value hasn't changed
	afterBody := fetchMetricsWithRetry(t, "http://127.0.0.1:9127/metrics", 20, 250*time.Millisecond)
	after, ok := findMetricValue(afterBody, "pg_doorman_pools_transactions_count", map[string]string{"user": user})
	if !ok {
		t.Fatalf("metric pg_doorman_pools_transactions_count{user=\"%s\"} not found after check query", user)
	}

	assert.Equal(t, before, after, "pg_doorman_pools_transactions_count should not change after ';'")
}
