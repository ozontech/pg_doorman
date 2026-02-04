package doorman_test

import (
	"database/sql"
	"os"
	"testing"
	"time"

	_ "github.com/lib/pq"
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
	// Check backend is in transaction
	assert.NoError(t, db.QueryRow("select count(*) from pg_stat_activity where usename = 'example_user_rollback' and state = 'idle in transaction'").Scan(&count))
	assert.Equal(t, 1, count)
	_, err = tx.Exec("aaaaaaa")
	assert.Error(t, err)
	// After error, backend is in aborted state until client sends ROLLBACK
	time.Sleep(100 * time.Millisecond)
	assert.NoError(t, db.QueryRow("select count(*) from pg_stat_activity where usename = 'example_user_rollback' and state = 'idle in transaction (aborted)'").Scan(&count))
	assert.Equal(t, 1, count)
	// Client explicitly rolls back
	_ = tx.Rollback()
	// After rollback, backend should not be in aborted state
	time.Sleep(100 * time.Millisecond)
	assert.NoError(t, db.QueryRow("select count(*) from pg_stat_activity where usename = 'example_user_rollback' and state = 'idle in transaction (aborted)'").Scan(&count))
	assert.Equal(t, 0, count)
}
