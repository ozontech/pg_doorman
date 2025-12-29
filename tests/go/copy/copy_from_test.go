package doorman_test

import (
	"context"
	"os"
	"testing"

    _ "github.com/lib/pq"
	"github.com/jackc/pgx/v4"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func Test_CopyFrom(t *testing.T) {
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer conn.Close(ctx)
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
}
