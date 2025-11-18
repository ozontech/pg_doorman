package doorman_test

import (
	"context"
	"os"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// TestCopyFrom verifies pgx.CopyFrom behavior and connection stability.
//
// What this test checks (in plain English):
// - We can successfully perform a bulk insert using pgx.CopyFrom (the PostgreSQL COPY protocol).
// - Using COPY does not cause the server to disconnect our session. We assert this by
//   comparing the backend PID (server process id) before and after COPY; if it stays
//   the same, the connection has not been dropped or re-established.
//
// Test flow:
// 1) Open a connection and capture the current backend PID.
// 2) Create a simple table and prepare a couple of rows.
// 3) Execute CopyFrom to load the rows via the COPY protocol.
// 4) Truncate the table to run one more simple command after COPY.
// 5) Compare the backend PID from step (1) with the current one — they must match.
func TestCopyFrom(t *testing.T) {
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer conn.Close(ctx)
	pidBefore := getBackendPid(ctx, conn, t)
	_, err = conn.Exec(ctx, "drop table if exists test_copy; create table test_copy (name text, age int);")
	require.NoError(t, err)
	rows := [][]interface{}{
		{"John", int32(36)},
		{"Jane", int32(29)},
	}
	_, err = conn.CopyFrom(
		context.Background(),
		pgx.Identifier{"test_copy"},
		[]string{"name", "age"},
		pgx.CopyFromRows(rows),
	)
	assert.NoError(t, err)
	_, err = conn.Exec(ctx, "truncate table test_copy;")
	assert.NoError(t, err)
	assert.Equal(t, pidBefore, getBackendPid(ctx, conn, t))
}
