package doorman_test

import (
	"context"
	"database/sql"
	"os"
	"testing"

	"github.com/jackc/pgx/v4"
	"github.com/stretchr/testify/assert"
)

func TestRollbackSavePoint(t *testing.T) {
	ctx := context.Background()
	session, errOpen := pgx.Connect(ctx, os.Getenv("DATABASE_URL_ROLLBACK"))
	assert.NoError(t, errOpen)
	tx, errTx := session.Begin(ctx)
	assert.NoError(t, errTx)
	tx.Exec(ctx, `select 1`)
	checkIdleInTx(t)
	_, err := tx.Exec(ctx, `
create table test_savepoint(id serial primary key, value integer);
insert into test_savepoint(value) values (1);
savepoint sp;
insert into test_savepoint(value) values (2);
`)
	assert.NoError(t, err)
	_, err = tx.Exec(ctx, `select * from test_savepoint_unknown;`)
	assert.Error(t, err)
	_, err = tx.Exec(ctx, `ROLLBACK TO SAVEPOINT sp;`)
	assert.NoError(t, err)
	checkIdleInTx(t)
	var count int
	assert.NoError(t, tx.QueryRow(ctx, `select count(*) from test_savepoint;`).Scan(&count))
	assert.Equal(t, count, 1)
	_, err = tx.Exec(ctx, `ROLLback /*kek*/;`)
	assert.NoError(t, err)
}

func checkIdleInTx(t *testing.T) {
	db, errOpenNew := sql.Open("postgres", os.Getenv("DATABASE_URL"))
	assert.NoError(t, errOpenNew)
	var count int
	assert.NoError(t, db.QueryRow("select count(*) from pg_stat_activity where usename = 'example_user_rollback' and state = 'idle in transaction'").Scan(&count))
	assert.Equal(t, 1, count)
	_ = db.Close()
}
