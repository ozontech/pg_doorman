package doorman_test

import (
	"crypto/tls"
	"net"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

const poolerAddr = "localhost:6433"

// TestCancelTLSQuery verifies that PostgreSQL CancelRequest (aka PQcancel/PqCancel)
// works correctly when the cancel request is sent over a TLS-encrypted connection
// to the pooler.
//
// Test outline:
// 1) Establish a plain TCP connection to the pooler for the main query (connQ),
//    authenticate via helper login() and start a long-running statement
//    (`select pg_sleep(10);`) using extended protocol messages.
// 2) Open a second TCP connection (connC) that will be used only to deliver the
//    cancel request. First, perform the PostgreSQL SSL negotiation by sending
//    an SSLRequest (int32 length=8, int32 code=80877103) and expect a single-byte
//    'S' response, indicating the server accepts TLS.
// 3) Upgrade connC to TLS using tls.Client and then send a CancelRequest message
//    (int32 length=16, int32 code=80877102, int32 processID, int32 secretKey)
//    with the process and secret obtained from the initial login(). Close the
//    TLS connection after writing the cancel.
// 4) Read from the original query connection (connQ) and assert that it returns
//    an error with SQLSTATE 57014 (query_canceled). The message is expected to
//    mention ProcessInterrupts, confirming the server processed the cancel.
//
// In short: this test proves that the pooler correctly handles SSL negotiation
// for the dedicated cancel connection and forwards the CancelRequest over TLS.
func TestCancelTLSQuery(t *testing.T) {
	// login
	connQ, errConnQ := net.Dial("tcp", poolerAddr)
	require.NoError(t, errConnQ)
	defer connQ.Close()
	t.Logf("connection query: address is %s", connQ.LocalAddr().String())
	processID, secretID := login(t, connQ, "example_user_1", "example_db", "test")
	{
		configureFlush(t, connQ, 100*time.Millisecond)
		t.Logf("connection query: send sleep\n")
		sendParseQuery(t, connQ, "select pg_sleep(10);")
		sendBindMessage(t, connQ)
		sendDescribe(t, connQ, "P")
		sendExecute(t, connQ)
		sendSyncMessage(t, connQ)
		time.Sleep(200 * time.Millisecond)
	}

	// send cancel.
	connC, errConnC := net.Dial("tcp", poolerAddr)
	require.NoError(t, errConnC)
	t.Logf("connection cancel: address is %s", connC.LocalAddr().String())
	defer connC.Close()

	{ // ssl
		t.Logf("connection cancel: send ssl request\n")
		pack := make([]byte, 0)
		pack = append(pack, i32ToBytes(8)...)
		pack = append(pack, i32ToBytes(80877103)...) // ssl request
		size, errW := connC.Write(pack)
		assert.NoError(t, errW)
		assert.Equal(t, len(pack), size)
		count, errR := connC.Read(pack)
		assert.NoError(t, errR)
		assert.Equal(t, count, 1)
		assert.Equal(t, string(pack[0]), "S")
	}

	{ // write cancel via tls.
		pgConnSSL := tls.Client(connC, &tls.Config{MinVersion: tls.VersionTLS12, InsecureSkipVerify: true})
		t.Logf("connection cancel (tls): write cancel")
		pack := make([]byte, 0)
		pack = append(pack, i32ToBytes(16)...)
		pack = append(pack, i32ToBytes(80877102)...) // cancel
		pack = append(pack, i32ToBytes(int32(processID))...)
		pack = append(pack, i32ToBytes(int32(secretID))...)
		count, errWrite := pgConnSSL.Write(pack)
		assert.NoError(t, errWrite)
		assert.Equal(t, len(pack), count)
		assert.Nil(t, pgConnSSL.Close())
	}

	{ // read response
		t.Logf("connection query: read sleep\n")
		pack := make([]byte, 10000)
		count, errRead := connQ.Read(pack)
		assert.NoError(t, errRead)
		assert.Contains(t, string(pack[:count]), "57014")
		assert.Contains(t, string(pack[:count]), "ProcessInterrupts")
	}
}

func configureFlush(t *testing.T, conn net.Conn, duration time.Duration) {
	tcpConn, ok := conn.(*net.TCPConn)
	assert.True(t, ok)
	assert.Nil(t, tcpConn.SetWriteDeadline(time.Now().Add(duration)))
}
