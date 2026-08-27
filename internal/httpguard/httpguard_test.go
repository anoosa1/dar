package httpguard

import (
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestLimitBodyRejectsOverflow(t *testing.T) {
	r := httptest.NewRequest(http.MethodPost, "/", strings.NewReader("12345"))
	LimitBody(httptest.NewRecorder(), r, 4)
	_, err := io.ReadAll(r.Body)
	var maxErr *http.MaxBytesError
	if !errors.As(err, &maxErr) || maxErr.Limit != 4 {
		t.Fatalf("read error = %v", err)
	}
}

func TestLimiterBoundsAndResetsKeys(t *testing.T) {
	l := NewLimiter(2, 10*time.Millisecond)
	if !l.Allow("client") || !l.Allow("client") || l.Allow("client") {
		t.Fatal("fixed-window limit was not enforced")
	}
	time.Sleep(15 * time.Millisecond)
	if !l.Allow("client") {
		t.Fatal("expired fixed window did not reset")
	}
	zero := NewLimiter(0, time.Minute)
	if zero.Allow("client") {
		t.Fatal("zero limit allowed a request")
	}
	bounded := NewLimiter(1, time.Hour)
	for i := 0; i < 4096; i++ {
		if !bounded.Allow(string(rune(i))) {
			t.Fatalf("entry %d unexpectedly denied", i)
		}
	}
	if bounded.Allow("overflow") || len(bounded.entries) != 4096 {
		t.Fatalf("limiter key cardinality=%d", len(bounded.entries))
	}
}
