package doorman_test

import (
	"context"
	"os"
	"testing"
	"time"

	"github.com/jackc/pgx/v4"
	"github.com/lib/pq"
	"github.com/stretchr/testify/assert"
)

func TestPGXV4(t *testing.T) {
	ctx := context.Background()
	db, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	assert.NoError(t, err)
	_, err = db.Exec(ctx, "drop table if exists lib_pq_users")
	assert.NoError(t, err)
	_, err = db.Exec(ctx, "create table lib_pq_users (id serial primary key, name text, dob date)")
	assert.NoError(t, err)
	tx, err := db.Begin(ctx)
	assert.NoError(t, err)
	_, err = tx.Exec(ctx, "insert into lib_pq_users (name, dob) values (unnest($1::text[]), unnest($2::date[]))",
		pq.Array([]string{"dmitrivasilyev", "alexign"}),
		pq.Array([]time.Time{
			time.Date(1983, 12, 12, 0, 0, 0, 0, time.UTC),
			time.Date(1981, 7, 19, 0, 0, 0, 0, time.UTC)}))
	assert.NoError(t, err)
	_, err = tx.Exec(ctx, "insert into lib_pq_users (name, dob) values (unnest($1::text[]), unnest($2::date[]))",
		pq.Array([]string{"dmitrivasilyev", "alexign"}),
		pq.Array([]time.Time{
			time.Date(1983, 12, 12, 0, 0, 0, 0, time.UTC),
			time.Date(1981, 7, 19, 0, 0, 0, 0, time.UTC)}))
	assert.NoError(t, err)
	assert.NoError(t, tx.Commit(ctx))
	_, err = db.Exec(ctx, "insert into lib_pq_users (name, dob) values (unnest($1::text[]), unnest($2::date[]))",
		pq.Array([]string{"dmitrivasilyev", "alexign"}),
		pq.Array([]time.Time{
			time.Date(1983, 12, 12, 0, 0, 0, 0, time.UTC),
			time.Date(1981, 7, 19, 0, 0, 0, 0, time.UTC)}))
	t.Log("pgx done")
}
