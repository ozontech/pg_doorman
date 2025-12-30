package doorman_test

import (
	"encoding/binary"
	"unicode/utf8"
)

func bytesToI32(bytes []byte) uint32 {
	return binary.BigEndian.Uint32(bytes)
}

func i32ToBytes(i int32) []byte {
	var arr [4]byte
	binary.BigEndian.PutUint32(arr[0:4], uint32(i))
	return arr[:]
}

func stringToBytes(in string) []byte {
	var rs []rune
	for _, r := range in {
		rs = append(rs, r)
	}
	bs := make([]byte, len(rs)*utf8.UTFMax)
	count := 0
	for _, r := range rs {
		count += utf8.EncodeRune(bs[count:], r)
	}
	bs = bs[:count]
	return bs
}
