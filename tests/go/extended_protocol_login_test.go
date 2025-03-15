package doorman_test

import (
	"crypto/md5"
	"encoding/hex"
	"fmt"
	"net"
	"testing"
	"time"
	"unicode/utf8"
)

func login(t *testing.T, conn net.Conn, username, database, password string) (processID int, secretKey int) {
	startup := make([]byte, 0)
	startup = append(startup, i32ToBytes(196608)...)    // 196608
	startup = append(startup, stringToBytes("user")...) // user
	startup = append(startup, []byte("\000")...)
	startup = append(startup, stringToBytes(username)...)
	startup = append(startup, []byte("\000")...)
	startup = append(startup, stringToBytes("database")...) // database
	startup = append(startup, []byte("\000")...)
	startup = append(startup, stringToBytes(database)...)
	startup = append(startup, []byte("\000")...)
	startup = append(startup, []byte("\000")...)
	startupMessageSize := int32(len(startup) + 4)
	startup = append(i32ToBytes(startupMessageSize), startup...)
	if count, err := conn.Write(startup); err != nil {
		t.Fatal(err)
	} else if count != int(startupMessageSize) {
		t.Fatal("wrong number of bytes written")
	}
	time.Sleep(500 * time.Millisecond)
	response := make([]byte, 5)
	t.Logf("read startup response\n")
	if count, err := conn.Read(response); err != nil {
		t.Fatal(err)
	} else if count != 5 {
		t.Fatal(fmt.Sprintf("expected 5 bytes read, but read: %d\n", count))
	}
	switch response[0] {
	case 'R':
		t.Logf("get R response\n")
	default:
		t.Fatal(fmt.Sprintf("expected R response, but got %v\n", string(response[0])))
	}
	response = make([]byte, 4)
	if count, err := conn.Read(response); err != nil {
		t.Fatal(err)
	} else if count != 4 {
		t.Fatal(fmt.Sprintf("expected 4 bytes read, but read: %d\n", count))
	}
	switch bytesToI32(response) {
	case 5: // md5
		t.Logf("get md5 response\n")
	case 0: // trust
		t.Logf("get trust response\n")
		return
	default:
		t.Fatal(fmt.Sprintf("%v -> read: %d\n", response, bytesToI32(response)))
	}
	// process md5.
	t.Logf("reading salt\n")
	salt := make([]byte, 4)
	if count, err := conn.Read(salt); err != nil {
		t.Fatal(err)
	} else if count != 4 {
		t.Fatal(fmt.Sprintf("expected 4 bytes read, but read: %d\n", count))
	}
	hash := md5.Sum([]byte(
		password + username))
	hash = md5.Sum([]byte(
		hex.EncodeToString(hash[:]) + string(salt)))
	md5Password := stringToBytes("md5" + hex.EncodeToString(hash[:]))
	md5Password = append(md5Password, "\000"...)
	passwordResponse := make([]byte, 1)
	utf8.EncodeRune(passwordResponse, 'p')
	passwordResponse = append(passwordResponse, i32ToBytes(int32(len(md5Password) + 4))[:]...)
	passwordResponse = append(passwordResponse, md5Password...)
	if count, err := conn.Write(passwordResponse); err != nil {
		t.Fatal(err)
	} else if count != len(passwordResponse) {
		t.Fatal(fmt.Sprintf("expected %d bytes read, but read: %d\n", len(passwordResponse), count))
	}
	response = make([]byte, 5)
	if count, err := conn.Read(response); err != nil {
		t.Fatal(err)
	} else if count != len(response) {
		t.Fatal(fmt.Sprintf("expected %d bytes read, but read: %d\n", len(response), count))
	}
	if string(response[0]) != "R" {
		t.Fatalf("password response: %v\n", string(response[0]))
	}
	response = make([]byte, bytesToI32(response[1:5])-4)
	if count, err := conn.Read(response); err != nil {
		t.Fatal(err)
	} else if count != len(response) {
		t.Fatal(fmt.Sprintf("expected %d bytes read, but read: %d\n", len(response), count))
	}
	for {
		t.Log("waiting for message")
		response = make([]byte, 5)
		if count, err := conn.Read(response); err != nil {
			t.Fatal(err)
		} else if count != len(response) {
			t.Fatal(fmt.Sprintf("expected %d bytes read, but read: %d\n", len(response), count))
		}
		switch response[0] {
		case 'S':
			response = make([]byte, bytesToI32(response[1:5])-4)
			if count, err := conn.Read(response); err != nil {
				t.Fatal(err)
			} else if count != len(response) {
				t.Fatal(fmt.Sprintf("expected %d bytes read, but read: %d\n", len(response), count))
			}
		case 'K':
			response = make([]byte, bytesToI32(response[1:5])-4)
			if count, err := conn.Read(response); err != nil {
				t.Fatal(err)
			} else if count != len(response) {
				t.Fatal(fmt.Sprintf("expected %d bytes read, but read: %d\n", len(response), count))
			}
			processID = int(bytesToI32(response[0:4]))
			secretKey = int(bytesToI32(response[4:8]))
			t.Logf("successfully get secret id: %d, process id %d", secretKey, processID)
		case 'Z':
			t.Logf("reading Z")
			response = make([]byte, 1)
			if count, err := conn.Read(response); err != nil {
				t.Fatal(err)
			} else if count != len(response) {
				t.Fatal(fmt.Sprintf("expected %d bytes read, but read: %d\n", len(response), count))
			}
			if response[0] != 'I' {
				t.Fatal("expected I after Z")
			}
			t.Logf("login done")
			return
		default:
			t.Logf("unexpected response: %v\n", string(response[0]))
		}
	}
	return
}
