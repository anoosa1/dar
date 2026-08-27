package testutil

import (
	"bufio"
	"net"
	"testing"
)

func FuzzWebSocketTextFrames(f *testing.F) {
	for _, seed := range []string{"{}", "hello", string(make([]byte, 126)), string(make([]byte, 65536))} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, raw string) {
		if len(raw) > 1<<20 {
			return
		}
		left, right := net.Pipe()
		defer left.Close()
		defer right.Close()
		writer := &webSocket{connection: left}
		reader := &webSocket{connection: right, reader: bufio.NewReader(right)}
		errs := make(chan error, 1)
		go func() {
			errs <- writer.writeText([]byte(raw))
		}()
		got, err := reader.readText()
		if err != nil {
			t.Fatal(err)
		}
		if string(got) != raw {
			t.Fatalf("frame round-trip length = %d, want %d", len(got), len(raw))
		}
		if err := <-errs; err != nil {
			t.Fatal(err)
		}
	})
}
