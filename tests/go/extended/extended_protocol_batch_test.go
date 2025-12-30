package doorman_test

import (
	"database/sql"
	"fmt"
	"net"
	"os"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func Test_SleepBatch(t *testing.T) {
	db, err := sql.Open("postgres", os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer db.Close()
	_, _ = db.Exec(`select pg_terminate_backend(pid)
from pg_stat_activity where query ~ 'pg_sleep' and not query ~ 'pg_stat_activity' and pid <> pg_backend_pid()`)
	sendBatchSleepWithCancel(t, 0, 100, 200)
}

func Test_ErrorBatch(t *testing.T) {
	sendBatchWithError(t)
}

func sendBatchSleepWithCancel(t *testing.T, _, first, second int) {
	conn, errConn := net.Dial("tcp", poolerAddr)
	if errConn != nil {
		t.Fatal(errConn)
	}
	defer conn.Close()
	processID, secretKey := login(t, conn, "example_user_1", "example_db", "test")
	t.Logf("processID: %d, secretKey: %d", processID, secretKey)
	{
		sendParseQuery(t, conn, fmt.Sprintf("select pg_sleep(%d)", first))
		sendBindMessage(t, conn)
		sendDescribe(t, conn, "P")
		sendExecute(t, conn)
	}
	{
		sendParseQuery(t, conn, fmt.Sprintf("select pg_sleep(%d)", second))
		sendBindMessage(t, conn)
		sendDescribe(t, conn, "P")
		sendExecute(t, conn)
	}
	sendSyncMessage(t, conn)
	time.Sleep(1 * time.Second) // we need time login to pg.
	now := time.Now()
	sendCancel(t, poolerAddr, processID, secretKey)
	messages := readServerMessages(t, conn)
	assert.Equal(t, 5, len(messages))
	assert.True(t, time.Since(now) < time.Second)
	byeBye(t, conn)
}

func sendBatchWithError(t *testing.T) {
	conn, errConn := net.Dial("tcp", poolerAddr)
	if errConn != nil {
		t.Fatal(errConn)
	}
	defer conn.Close()
	processID, secretKey := login(t, conn, "example_user_1", "example_db", "test")
	t.Logf("processID: %d, secretKey: %d", processID, secretKey)
	{
		sendParseQuery(t, conn, fmt.Sprintf("select 1"))
		sendBindMessage(t, conn)
		sendDescribe(t, conn, "P")
		sendExecute(t, conn)
	}
	{
		sendParseQuery(t, conn, fmt.Sprintf("select sasasa"))
		sendBindMessage(t, conn)
		sendDescribe(t, conn, "P")
		sendExecute(t, conn)
	}
	sendSyncMessage(t, conn)
	messages := readServerMessages(t, conn)
	assert.Equal(t, 7, len(messages))
	{
		sendParseQuery(t, conn, fmt.Sprintf("SELECT * FROM generate_series(1,1000)"))
		sendBindMessage(t, conn)
		sendDescribe(t, conn, "P")
		sendExecute(t, conn)
		sendSyncMessage(t, conn)
		assert.Equal(t, 1006, len(readServerMessages(t, conn)))
	}
	byeBye(t, conn)
}

func sendCancel(t *testing.T, addr string, processID, secretKey int) {
	connC, errConnC := net.Dial("tcp", addr)
	require.NoError(t, errConnC)
	defer connC.Close()
	t.Logf("connection cancel: send cancel")
	pack := make([]byte, 0)
	pack = append(pack, i32ToBytes(16)...)
	pack = append(pack, i32ToBytes(80877102)...) // cancel
	pack = append(pack, i32ToBytes(int32(processID))...)
	pack = append(pack, i32ToBytes(int32(secretKey))...)
	count, errWrite := connC.Write(pack)
	assert.NoError(t, errWrite)
	assert.Equal(t, len(pack), count)
	assert.Nil(t, connC.Close())
}
