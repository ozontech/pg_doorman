package doorman_test

import (
	"database/sql"
	"os"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
)

func Test_Rollback(t *testing.T) {
	db, errOpen := sql.Open("postgres", os.Getenv("DATABASE_URL_ROLLBACK"))
	assert.NoError(t, errOpen)
	defer db.Close()
	_, err := db.Exec("select pg_terminate_backend(pid) from pg_stat_activity where usename = 'example_user_rollback' and pid <> pg_backend_pid()")
	assert.NoError(t, err)
	tx, errTx := db.Begin()
	assert.NoError(t, errTx)
	_, err = tx.Exec(`select 1`)
	assert.NoError(t, err)
	var count int
	// 2 backends in non-idle
	assert.NoError(t, db.QueryRow("select count(*) from pg_stat_activity where usename = 'example_user_rollback' and state = 'idle in transaction'").Scan(&count))
	assert.Equal(t, 1, count)
	_, err = tx.Exec("aaaaaaa")
	assert.Error(t, err)
	// auto-rollback
	time.Sleep(100 * time.Millisecond)
	assert.NoError(t, db.QueryRow("select count(*) from pg_stat_activity where usename = 'example_user_rollback' and state = 'idle in transaction (aborted)'").Scan(&count))
	assert.Equal(t, 0, count)
	_ = tx.Rollback()
}
