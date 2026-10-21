package doorman_test

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"fmt"
	"net"
	"net/url"
	"os"
	"strings"
	"testing"

	"github.com/stretchr/testify/require"
	"golang.org/x/crypto/pbkdf2"
)

// Regression test for https://github.com/ozontech/pg_doorman/pull/282
//
// pg_doorman master parses the SASLInitialResponse with
// String::from_utf8_lossy() and then slices it at a fixed byte offset.
// When the SCRAM client-first-message is 128 bytes or longer, the 4-byte
// big-endian response length contains a byte >= 0x80, which is not valid
// standalone UTF-8. from_utf8_lossy replaces it with U+FFFD, the offsets
// shift, and authentication fails with SQLSTATE 28P01 even though the
// credentials are correct.
//
// This test is a real client: it performs a complete SCRAM-SHA-256
// exchange over TCP with a deliberately long nonce, so the
// client-first-message payload is exactly 128 bytes.
// It fails on master and passes with PR #282 applied.

func readMessage(t *testing.T, conn net.Conn) (byte, []byte) {
	t.Helper()
	header := make([]byte, 5)
	_, err := readFull(conn, header)
	require.NoError(t, err)
	length := binary.BigEndian.Uint32(header[1:5])
	require.GreaterOrEqual(t, length, uint32(4))
	payload := make([]byte, length-4)
	_, err = readFull(conn, payload)
	require.NoError(t, err)
	return header[0], payload
}

func readFull(conn net.Conn, buf []byte) (int, error) {
	total := 0
	for total < len(buf) {
		n, err := conn.Read(buf[total:])
		if err != nil {
			return total, err
		}
		total += n
	}
	return total, nil
}

func writeMessage(conn net.Conn, msgType byte, payload []byte) error {
	buf := make([]byte, 5+len(payload))
	buf[0] = msgType
	binary.BigEndian.PutUint32(buf[1:5], uint32(4+len(payload)))
	copy(buf[5:], payload)
	_, err := conn.Write(buf)
	return err
}

func parsePgError(payload []byte) string {
	// ErrorResponse fields: 1-byte code, string, \0, terminated by \0.
	code, detail := "", ""
	for i := 0; i+1 < len(payload) && payload[i] != 0; {
		field := payload[i]
		i++
		end := i
		for end < len(payload) && payload[end] != 0 {
			end++
		}
		value := string(payload[i:end])
		i = end + 1
		if field == 'C' {
			code = value
		}
		if field == 'M' {
			detail = value
		}
	}
	return code + ": " + detail
}

func TestServerAuthSCRAMLongNonce(t *testing.T) {
	rawURL := os.Getenv("DATABASE_URL_SCRAM_LONG_NONCE")
	require.NotEmpty(t, rawURL, "DATABASE_URL_SCRAM_LONG_NONCE must be set")
	u, err := url.Parse(rawURL)
	require.NoError(t, err)
	password, _ := u.User.Password()
	require.NotEmpty(t, password)

	conn, err := net.Dial("tcp", u.Host)
	require.NoError(t, err)
	defer conn.Close()

	// Startup message (protocol 3.0).
	user := u.User.Username()
	db := strings.TrimPrefix(u.Path, "/")
	startup := []byte{}
	startup = append(startup, 0, 3, 0, 0) // protocol 196608
	startup = append(startup, "user"...)
	startup = append(startup, 0)
	startup = append(startup, user...)
	startup = append(startup, 0)
	startup = append(startup, "database"...)
	startup = append(startup, 0)
	startup = append(startup, db...)
	startup = append(startup, 0, 0)
	lenBuf := make([]byte, 4)
	binary.BigEndian.PutUint32(lenBuf, uint32(len(startup)+4))
	_, err = conn.Write(append(lenBuf, startup...))
	require.NoError(t, err)

	// Expect AuthenticationSASL (R, subtype 10) offering SCRAM-SHA-256.
	msgType, payload := readMessage(t, conn)
	if msgType == 'E' {
		t.Fatalf("server rejected startup: %s", parsePgError(payload))
	}
	require.Equal(t, byte('R'), msgType, "expected auth request, got %q", msgType)
	require.Equal(t, uint32(10), binary.BigEndian.Uint32(payload[:4]), "expected SASL auth request")
	require.Contains(t, string(payload[4:]), "SCRAM-SHA-256")

	// Long nonce: client-first-message "n,,n=,r=<nonce>" is exactly
	// 128 bytes, so the SASL response length is 0x00000080 and master
	// corrupts it with from_utf8_lossy.
	nonceBytes := make([]byte, 120)
	_, err = rand.Read(nonceBytes)
	require.NoError(t, err)
	nonce := base64.RawStdEncoding.EncodeToString(nonceBytes)[:120]
	clientFirstBare := "n=,r=" + nonce
	clientFirst := "n,," + clientFirstBare
	require.Equal(t, 128, len(clientFirst))

	saslInit := append([]byte("SCRAM-SHA-256"), 0)
	lenField := make([]byte, 4)
	binary.BigEndian.PutUint32(lenField, uint32(len(clientFirst)))
	saslInit = append(saslInit, lenField...)
	saslInit = append(saslInit, clientFirst...)
	require.NoError(t, writeMessage(conn, 'p', saslInit))

	// Expect AuthenticationSASLContinue (R, subtype 11) with server-first-message.
	msgType, payload = readMessage(t, conn)
	if msgType == 'E' {
		t.Fatalf("server rejected client-first-message (bug #282): %s", parsePgError(payload))
	}
	require.Equal(t, byte('R'), msgType)
	require.Equal(t, uint32(11), binary.BigEndian.Uint32(payload[:4]), "expected SASL continue")
	serverFirst := string(payload[4:])

	var serverNonce, saltB64 string
	var iterations int
	for _, part := range strings.Split(serverFirst, ",") {
		switch {
		case strings.HasPrefix(part, "r="):
			serverNonce = part[2:]
		case strings.HasPrefix(part, "s="):
			saltB64 = part[2:]
		case strings.HasPrefix(part, "i="):
			_, err := fmt.Sscanf(part[2:], "%d", &iterations)
			require.NoError(t, err)
		}
	}
	require.True(t, strings.HasPrefix(serverNonce, nonce), "server nonce must extend client nonce")
	salt, err := base64.StdEncoding.DecodeString(saltB64)
	require.NoError(t, err)
	require.Positive(t, iterations)

	// Client-final-message.
	channelBinding := "c=biws"
	clientFinalNoProof := channelBinding + ",r=" + serverNonce
	authMessage := clientFirstBare + "," + serverFirst + "," + clientFinalNoProof

	saltedPassword := pbkdf2.Key([]byte(password), salt, iterations, 32, sha256.New)
	h := hmac.New(sha256.New, saltedPassword)
	h.Write([]byte("Client Key"))
	clientKey := h.Sum(nil)
	storedKey := sha256.Sum256(clientKey)
	h = hmac.New(sha256.New, storedKey[:])
	h.Write([]byte(authMessage))
	clientSignature := h.Sum(nil)
	proof := make([]byte, 32)
	for i := range proof {
		proof[i] = clientKey[i] ^ clientSignature[i]
	}
	clientFinal := clientFinalNoProof + ",p=" + base64.StdEncoding.EncodeToString(proof)
	require.NoError(t, writeMessage(conn, 'p', []byte(clientFinal)))

	// Expect AuthenticationSASLFinal (R, subtype 12), then AuthenticationOk.
	msgType, payload = readMessage(t, conn)
	if msgType == 'E' {
		t.Fatalf("server rejected client-final-message: %s", parsePgError(payload))
	}
	require.Equal(t, byte('R'), msgType)
	require.Equal(t, uint32(12), binary.BigEndian.Uint32(payload[:4]), "expected SASL final")

	// Verify ServerSignature.
	serverFinal := string(payload[4:])
	require.True(t, strings.HasPrefix(serverFinal, "v="))
	serverSig, err := base64.StdEncoding.DecodeString(serverFinal[2:])
	require.NoError(t, err)
	h = hmac.New(sha256.New, saltedPassword)
	h.Write([]byte("Server Key"))
	serverKey := h.Sum(nil)
	h = hmac.New(sha256.New, serverKey)
	h.Write([]byte(authMessage))
	require.True(t, hmac.Equal(serverSig, h.Sum(nil)), "bad server signature")

	msgType, _ = readMessage(t, conn)
	require.Equal(t, byte('R'), msgType, "expected AuthenticationOk")

	// A simple query proves the pooled backend connection works end to end.
	require.NoError(t, writeMessage(conn, 'Q', append([]byte("select 1"), 0)))
	gotReady := false
	for !gotReady {
		msgType, payload = readMessage(t, conn)
		if msgType == 'E' {
			t.Fatalf("query failed: %s", parsePgError(payload))
		}
		if msgType == 'Z' {
			gotReady = true
		}
	}
}
