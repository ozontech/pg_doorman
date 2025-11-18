package doorman_test

import (
	"database/sql"
	"os"
	"testing"

	"github.com/stretchr/testify/assert"
)

// TestCopy verifies that a COPY FROM stdin operation respects the session's statement_timeout.
//
// What the test does:
// 1) Opens a DB connection and prepares a simple table: test_copy(t text).
// 2) Starts a separate transaction (txLock) that locks the table with "lock table test_copy" and
//    keeps it locked until we explicitly release it. This simulates a long‑running lock that would
//    block any concurrent write to the table.
// 3) In another transaction (txCopy), sets a short local statement timeout (1s) and attempts to
//    run "COPY test_copy(t) FROM stdin". Because the table is locked by txLock, the COPY command
//    cannot acquire the needed lock and is forced to wait. The 1-second timeout elapses, so COPY
//    must fail with a timeout error.
// 4) The test asserts that an error is returned by COPY (the expected behavior under timeout), and
//    then rolls back txCopy. After that, it signals the first goroutine to commit txLock, releasing
//    the table lock and finishing the test cleanly.
//
// In short: COPY is blocked by a table lock; with a short statement_timeout it should fail quickly,
// and the test checks exactly that.
func TestCopy(t *testing.T) {
	db, errOpen := sql.Open("postgres", os.Getenv("DATABASE_URL"))
	assert.NoError(t, errOpen)
	defer db.Close()
	// prepare
	{
		_, errExec := db.Exec("drop table if exists test_copy; create table test_copy(t text);")
		assert.NoError(t, errExec)
	}
	done := make(chan struct{}, 1)
	sync := make(chan struct{}, 1)
	// run tx with lock.
	{
		txLock, errTxLock := db.Begin()
		assert.NoError(t, errTxLock)
		_, errExec := txLock.Exec("lock table test_copy")
		assert.NoError(t, errExec)
		go func() {
			<-sync
			_ = txLock.Commit()
			done <- struct{}{}
		}()
	}
	// run with timeout
	{
		txCopy, errTxCopy := db.Begin()
		assert.NoError(t, errTxCopy)
		_, errExec := txCopy.Exec("set local statement_timeout to '1s'")
		assert.NoError(t, errExec)
		_, errExec = txCopy.Exec("COPY test_copy(t) FROM stdin")
		assert.Error(t, errExec)
		assert.NoError(t, txCopy.Rollback())
		sync <- struct{}{}
	}
	<-done
}
