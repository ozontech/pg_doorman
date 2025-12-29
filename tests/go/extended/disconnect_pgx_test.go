package doorman_test

// This test verifies the "disconnect and recover" behavior expected with pg_doorman.
// Goal in plain words:
//   - When a client loses the TCP connection in the middle of a query, the server-side
//     backend connection should be returned to the backend pool in a clean state,
//     ready to be reused by the next client without any leftovers from the previous session.
//
// How we simulate it here with pgx:
//   1) We build a pgx connection config and override DialFunc.
//      - On the first dial, we connect normally but immediately set a short deadline on the TCP connection.
//        This forces a read/write timeout while the query is in progress, simulating a broken client socket.
//      - On any subsequent dial attempt by the same client (e.g., for cancel), we return an error to ignore it —
//        we want the server-side to proceed and produce a response as if the client vanished, not to cancel.
//   2) We run a query that guarantees the deadline will elapse mid-flight: a small sleep plus some result rows.
//      The client receives a timeout error and drops its socket.
//   3) After a short wait, we connect again and check the original backend PID in pg_stat_activity.
//      Two acceptable outcomes prove the backend was recovered correctly:
//        - Either the same PID is still there as "active" serving our diagnostic SELECT (with the query marker), or
//        - The previous PID is gone and the connection that replaced it is "idle" and waiting (Client/ClientRead),
//          meaning it's clean and reusable in the pool.

import (
	"context"
	"database/sql"
	"errors"
	"net"
	"os"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

const (
	clientDeadline   = 50 * time.Millisecond  // deadline to force mid-query timeout on the client socket
	serverThinkTime  = 200 * time.Millisecond // time for server/proxy to finish & recycle backend
	diagnosticMarker = "/*Test_Disconnect*/"  // marker to find our check query in pg_stat_activity
)

func Test_Disconnect(t *testing.T) {
	// Table-driven subtests for two scenarios: inside a transaction and a single statement
	tcs := []struct {
		name   string
		before func(context.Context, *pgx.Conn) error
	}{
		{
			name: "with transaction",
			before: func(ctx context.Context, session *pgx.Conn) error {
				_, err := session.Exec(ctx, "BEGIN;")
				return err
			},
		},
		{
			name: "with simple query",
			before: func(ctx context.Context, session *pgx.Conn) error {
				_, err := session.Exec(ctx, ";")
				return err
			},
		},
	}

	for _, tc := range tcs {
		// capture range variable
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			testDisconnectWithCustomFunc(t, tc.before)
		})
	}
}

func testDisconnectWithCustomFunc(t *testing.T, fnBefore func(context.Context, *pgx.Conn) error) {
	t.Helper()
	ctx := context.Background()
	pidBefore := runDisconnectWithCustomFunc(t, fnBefore)
	t.Logf("pid before: %d", pidBefore)

	// check the current server pid, it must be the same as before disconnect, or it must be idle.
	session, errOpen := pgx.Connect(ctx, os.Getenv("DATABASE_URL_DISCONNECT"))
	require.NoError(t, errOpen)
	t.Cleanup(func() { _ = session.Close(ctx) })

	var backendPid sql.NullInt32
	var state, waitEvent, waitEventType, query sql.NullString
	require.NoError(t, session.QueryRow(ctx, `select `+diagnosticMarker+` state, wait_event, wait_event_type, query, pg_backend_pid() from pg_stat_activity where pid = $1`, pidBefore).Scan(
		&state, &waitEvent, &waitEventType, &query, &backendPid))

	t.Logf("pid after: %d", backendPid.Int32)
	if backendPid.Int32 == pidBefore {
		assert.Equal(t, "active", state.String)
		assert.Equal(t, "", waitEventType.String)
		assert.Equal(t, "", waitEvent.String)
		assert.Contains(t, query.String, diagnosticMarker)
	} else {
		t.Logf("pid state: state=%q wait_event_type=%q wait_event=%q", state.String, waitEventType.String, waitEvent.String)
		assert.Equal(t, "idle", state.String)
		assert.Equal(t, "Client", waitEventType.String)
		assert.Equal(t, "ClientRead", waitEvent.String)
	}
}

func runDisconnectWithCustomFunc(t *testing.T, fnBefore func(context.Context, *pgx.Conn) error) int32 {
	t.Helper()
	ctx := context.Background()
	config, parseConfig := pgx.ParseConfig(os.Getenv("DATABASE_URL_DISCONNECT"))
	require.NoError(t, parseConfig)

	config.DefaultQueryExecMode = pgx.QueryExecModeSimpleProtocol

	// Allow only the first dial to succeed; subsequent dials (e.g., cancel) should fail to simulate a vanished client.
	onlyFirst := true
	config.DialFunc = func(_ context.Context, network, addr string) (net.Conn, error) {
		if !onlyFirst {
			return nil, errors.New("this is cancel dial func")
		}
		d := &net.Dialer{}
		c, err := d.DialContext(context.Background(), network, addr)
		if err != nil {
			return nil, err
		}
		assert.NoError(t, c.SetDeadline(time.Now().Add(clientDeadline)))
		onlyFirst = false
		return c, nil
	}

	session, errOpen := pgx.ConnectConfig(ctx, config)
	require.NoError(t, errOpen)
	// No explicit Close: the intent is to simulate a client socket timeout mid-flight.

	require.NoError(t, fnBefore(ctx, session))
	pidBefore := getBackendPid(ctx, session, t)

	// This pair guarantees the client-side deadline will hit during execution.
	_, err := session.Exec(ctx, "select pg_sleep(0.2); select generate_series(1, 10000);")
	assert.Error(t, err)
	assert.Contains(t, err.Error(), "timeout")

	// Give the server/proxy some time to finalize and recycle the backend connection.
	time.Sleep(serverThinkTime)
	return pidBefore
}

func getBackendPid(ctx context.Context, db *pgx.Conn, t *testing.T) int32 {
	t.Helper()
	var pid int32
	err := db.QueryRow(ctx, "select pg_backend_pid()").Scan(&pid)
	require.NoError(t, err)
	return pid
}
