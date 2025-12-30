package doorman_test

import (
	"context"
	"os"
	"testing"

	"github.com/jackc/pgx/v4/pgxpool"
	"github.com/stretchr/testify/assert"
)

func TestDeallocate(t *testing.T) {
	ctx := context.Background()
	db, err := pgxpool.Connect(ctx, os.Getenv("DATABASE_URL"))
	assert.NoError(t, err)
	_, err = db.Exec(ctx, "deallocate \"test\"")
	assert.NoError(t, err)
	db.Close()
}
