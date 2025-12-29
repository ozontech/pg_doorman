package doorman_test

import (
	"bytes"
	"net"
	"testing"
)

type message struct {
	code   rune
	length uint32
	bytes  []byte
}

func mustEqualMessages(t *testing.T, a, b *message) {
	if string(a.code) != string(b.code) {
		t.Errorf("message are not equal. got %s want %s", string(a.code), string(b.code))
		return
	}
	if a.length != b.length {
		t.Errorf("message length are not equal. got %d want %d", a.length, b.length)
		return
	}
	if !bytes.Equal(a.bytes, b.bytes) {
		t.Errorf("message are not equal. got %s want %s", string(a.bytes), string(b.bytes))
		return
	}
	t.Logf("message is equal. got %s want %s", string(a.code), string(b.code))
}

func readServerMessages(t *testing.T, conn net.Conn) []*message {
	var messages []*message
	for {
		response := make([]byte, 5)
		readAll(t, conn, response)
		code, length := response[0], bytesToI32(response[1:5])
		bb := make([]byte, length-4)
		readAll(t, conn, bb)
		messages = append(messages, &message{code: rune(code), length: length, bytes: bb})
		if code == 'Z' {
			return messages
		}
	}
}

func readAll(t *testing.T, conn net.Conn, buf []byte) {
	from := 0
	for {
		count, err := conn.Read(buf[from:])
		if err != nil {
			t.Fatal(err)
		}
		if count == len(buf) {
			return
		}
		from += count
	}
}
