package doorman_test

import (
	"context"
	"os"
	"testing"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/stretchr/testify/assert"
)

func TestDeallocate(t *testing.T) {
	ctx := context.Background()
	db, err := pgxpool.New(ctx, os.Getenv("DATABASE_URL"))
	assert.NoError(t, err)
	_, err = db.Exec(ctx, "deallocate \"test\"")
	assert.NoError(t, err)
	db.Close()
}
