package doorman_test

import (
	"database/sql"
	"net"
	"testing"
	"time"

	_ "github.com/lib/pq"
	"github.com/stretchr/testify/assert"
)

const directPgDSN = `user=postgres host=localhost port=5432 sslmode=disable`

func printStaledConnections(t *testing.T) {
	db, err := sql.Open("postgres", directPgDSN)
	assert.NoError(t, err)
	defer db.Close()
	rows, errRows := db.Query(`select query::text, state::text, wait_event, wait_event_type from pg_stat_activity where usename = 'example_user_1' and state <> 'idle' and pid <> pg_backend_pid()`)
	assert.NoError(t, errRows)
	for rows.Next() {
		var query, state string
		var waitEventType, waitEvent sql.NullString
		assert.NoError(t, rows.Scan(&query, &state, &waitEvent, &waitEventType))
		t.Logf("query: %s, state: `%s`, wait_event: `%s`, wait_event_type: `%s`\n",
			query, state, waitEvent.String, waitEventType.String)
	}
	assert.NoError(t, rows.Close())
}

func checkStaledConnections(t *testing.T) {
	db, err := sql.Open("postgres", directPgDSN)
	defer db.Close()
	assert.NoError(t, err)
	count := 0
	for {
		var staled int
		assert.NoError(t, db.QueryRow(`select count(*) from pg_stat_activity
		        where usename = 'example_user_1' and state <> 'idle' and pid <> pg_backend_pid()`).Scan(
			&staled))
		if staled == 0 {
			return
		}
		count++
		printStaledConnections(t)
		if count > 100 {
			assert.Equal(t, staled, 0)
			t.Fatal("staled connections were not cleaned up")
		}
		t.Logf("staled connections: %d, sleeping\n", staled)

		time.Sleep(5 * time.Second)
	}
}

func Test_RaceStop(t *testing.T) {
	t.Log("start RaceStop")
	printStaledConnections(t)
	conn, errConn := net.Dial("tcp", poolerAddr)
	if errConn != nil {
		t.Fatal(errConn)
	}
	_, _ = login(t, conn, "example_user_1", "example_db", "test")
	sendParseQuery(t, conn, "SELECT * FROM generate_series(1,1000000);")
	time.Sleep(100 * time.Millisecond)
	sendBindMessage(t, conn)
	sendDescribe(t, conn, "P")
	sendExecute(t, conn)
	sendParseQuery(t, conn, "select pg_sleep(1)")
	byeBye(t, conn)
	if err := conn.Close(); err != nil {
		t.Error(err)
	}
	count := 0
	concurrency := make(chan struct{}, 10)
	for {
		concurrency <- struct{}{}
		go func() {
			conn, errConn := net.Dial("tcp", poolerAddr)
			if errConn != nil {
				t.Error(errConn)
				return
			}
			_, _ = login(t, conn, "example_user_1", "example_db", "test")
			sendParseQuery(t, conn, "SELECT * FROM generate_series(1,1000000);")
			time.Sleep(100 * time.Millisecond)
			sendBindMessage(t, conn)
			sendDescribe(t, conn, "P")
			sendExecute(t, conn)
			if count%2 == 0 {
				sendSyncMessage(t, conn)
			} else {
				byeBye(t, conn)
			}
			if err := conn.Close(); err != nil {
				t.Error(err)
			}
			<-concurrency
		}()
		count++
		if count > 100 {
			break
		}
	}
	checkStaledConnections(t)
}

func Test_RaceExtendedProtocol(t *testing.T) {
	t.Log("start RaceExtendedProtocol")
	printStaledConnections(t)
	conn, errConn := net.Dial("tcp", poolerAddr)
	if errConn != nil {
		t.Fatal(errConn)
	}
	_, _ = login(t, conn, "example_user_1", "example_db", "test")
	sendSimpleQuery(t, conn, "begin;")
	readServerMessages(t, conn)
	sendParseQuery(t, conn, "SELECT * FROM generate_series(1,1000000);")
	time.Sleep(100 * time.Millisecond)
	sendBindMessage(t, conn)
	sendDescribe(t, conn, "P")
	sendExecute(t, conn)
	sendParseQuery(t, conn, "select pg_sleep(1)")
	time.Sleep(100 * time.Millisecond)
	sendBindMessage(t, conn)
	sendDescribe(t, conn, "P")
	sendExecute(t, conn)
	sendSyncMessage(t, conn)
	time.Sleep(100 * time.Millisecond)
	read := make([]byte, 1)
	_, err := conn.Read(read)
	if err != nil {
		t.Fatal(err)
	}
	time.Sleep(2 * time.Second)
	byeBye(t, conn)
	assert.Nil(t, conn.Close())
	checkStaledConnections(t)
}

func Test_ExtendedProtocol(t *testing.T) {
	f := func(t *testing.T, conn net.Conn) []*message {
		processID, secretKey := login(t, conn, "example_user_1", "example_db", "test")
		t.Logf("processID: %d, secretKey: %d\n", processID, secretKey)
		if processID == 0 {
			_ = readServerMessages(t, conn)
		}
		sendParseQuery(t, conn, "select pg_sleep(0.1)")
		sendBindMessage(t, conn)
		sendDescribe(t, conn, "P")
		sendExecute(t, conn)
		sendParseQuery(t, conn, "select 1")
		sendBindMessage(t, conn)
		sendDescribe(t, conn, "P")
		sendExecute(t, conn)
		sendSyncMessage(t, conn)
		messages := readServerMessages(t, conn)
		byeBye(t, conn)
		return messages
	}
	doorman := getMessages(t, poolerAddr, f)
	pg := getMessages(t, "localhost:5432", f)
	if len(pg) != len(doorman) {
		t.Fatalf("got %d messages from pg, expected %d messages from doorman", len(pg), len(doorman))
	}
	for i, msgDoorman := range doorman {
		msgPg := pg[i]
		mustEqualMessages(t, msgPg, msgDoorman)
	}
}

func getMessages(t *testing.T, address string, f func(t *testing.T, conn net.Conn) []*message) []*message {
	conn, errConn := net.Dial("tcp", address)
	if errConn != nil {
		t.Fatal(errConn)
	}
	defer conn.Close()
	return f(t, conn)
}
