package doorman_test

import (
	"context"
	"os"
	"testing"

	"github.com/jackc/pgx/v4/pgxpool"
	"github.com/stretchr/testify/assert"
)

// TestDeallocate verifies that issuing `DEALLOCATE "test"` does not produce an error,
// even if there is no previously prepared statement named "test". The server should
// accept deallocating a non-existent prepared statement without complaining.
func TestDeallocate(t *testing.T) {
	ctx := context.Background()
	db, err := pgxpool.Connect(ctx, os.Getenv("DATABASE_URL"))
	assert.NoError(t, err)
	_, err = db.Exec(ctx, "deallocate \"test\"")
	assert.NoError(t, err)
	db.Close()
}
