package doorman_test

import (
	"net"
	"testing"
	"unicode/utf8"
)

func byeBye(t *testing.T, conn net.Conn) {
	message := make([]byte, 1)
	utf8.EncodeRune(message, 'X')
	message = append(message, i32ToBytes(4)...)
	if count, err := conn.Write(message); err != nil {
		t.Fatal(err)
	} else if count != len(message) {
		t.Fatal("expected to write", len(message), "but got", count)
	}
	t.Logf("send terminate: bye bye")
}

func sendSyncMessage(t *testing.T, conn net.Conn) {
	message := make([]byte, 1)
	utf8.EncodeRune(message, 'S')
	message = append(message, i32ToBytes(4)...)
	if count, err := conn.Write(message); err != nil {
		t.Fatal(err)
	} else if count != len(message) {
		t.Fatal("expected to write", len(message), "but got", count)
	}
	t.Log("successfully send sync")
}

func sendExecute(t *testing.T, conn net.Conn) {
	message := make([]byte, 1)
	utf8.EncodeRune(message, 'E')
	message = append(message, i32ToBytes(9)...)
	message = append(message, "\000"...) // unnamed statement
	message = append(message, i32ToBytes(0)...)
	if count, err := conn.Write(message); err != nil {
		t.Fatal(err)
	} else if count != len(message) {
		t.Fatal("expected to write", len(message), "but got", count)
	}
	t.Log("successfully send execute")
}

func sendSimpleQuery(t *testing.T, conn net.Conn, query string) {
	message := make([]byte, 1)
	utf8.EncodeRune(message, 'Q')
	message = append(message, i32ToBytes(int32(len(query)+4+1))...)
	message = append(message, stringToBytes(query)...)
	message = append(message, "\000"...)
	if count, err := conn.Write(message); err != nil {
		t.Fatal(err)
	} else if count != len(message) {
		t.Fatal("expected to write", len(message), "but got", count)
	}
	t.Logf("successfully query: %s\n", query)
}

func sendParseQuery(t *testing.T, conn net.Conn, query string) {
	messageSize := 2 + 2 + 4 + len(query)
	message := make([]byte, 1)
	utf8.EncodeRune(message, 'P')
	message = append(message, i32ToBytes(int32(messageSize))...)
	message = append(message, "\000"...) // unnamed statement
	message = append(message, stringToBytes(query)...)
	message = append(message, "\000"...) // close query
	message = append(message, "\000"...)
	message = append(message, "\000"...)
	if count, err := conn.Write(message); err != nil {
		t.Fatal(err)
	} else if count != len(message) {
		t.Fatal("expected to write", len(message), "but got", count)
	}
	t.Logf("successfully parse: %s\n", query)
}

func sendDescribe(t *testing.T, conn net.Conn, mode string) {
	message := make([]byte, 1)
	utf8.EncodeRune(message, 'D')
	message = append(message, i32ToBytes(6)...)
	message = append(message, stringToBytes(mode)...)
	message = append(message, "\000"...) // unnamed statement
	if count, err := conn.Write(message); err != nil {
		t.Fatal(err)
	} else if count != len(message) {
		t.Fatal("expected to write", len(message), "but got", count)
	}
	t.Logf("successfully describe: %s\n", mode)
}

func sendBindMessage(t *testing.T, conn net.Conn) {
	message := make([]byte, 1)
	utf8.EncodeRune(message, 'B')
	message = append(message, i32ToBytes(12)...)
	message = append(message, "\000"...) // unnamed statement
	message = append(message, "\000"...) // unnamed statement
	message = append(message, "\000\000\000\000\000\000"...)
	if count, err := conn.Write(message); err != nil {
		t.Fatal(err)
	} else if count != len(message) {
		t.Fatal("expected to write", len(message), "but got", count)
	}
	t.Logf("successfully bind\n")
}
