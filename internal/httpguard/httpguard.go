// Package httpguard centralizes finite request limits and in-memory rate limiting.
package httpguard

import (
	"net/http"
	"sync"
	"time"
)

const (
	FilesBody   int64 = 128 << 20
	EmailBody   int64 = 25 << 20
	ObjectBody  int64 = 10 << 20
	JSONBody    int64 = 2 << 20
	CardDAVBody int64 = 1 << 20
)

// LimitBody installs a hard request body ceiling. Read attempts beyond the
// limit return http.MaxBytesError.
func LimitBody(w http.ResponseWriter, r *http.Request, limit int64) {
	r.Body = http.MaxBytesReader(w, r.Body, limit)
}

type bucket struct {
	started time.Time
	count   int
}

// Limiter is a fixed-window in-memory limiter suitable for per-process abuse controls.
type Limiter struct {
	mu      sync.Mutex
	limit   int
	window  time.Duration
	entries map[string]bucket
}

func NewLimiter(limit int, window time.Duration) *Limiter {
	return &Limiter{limit: limit, window: window, entries: map[string]bucket{}}
}

func (l *Limiter) Allow(key string) bool {
	now := time.Now()
	l.mu.Lock()
	defer l.mu.Unlock()
	if _, exists := l.entries[key]; !exists && len(l.entries) >= 4096 {
		for k, old := range l.entries {
			if now.Sub(old.started) >= l.window {
				delete(l.entries, k)
			}
		}
		if len(l.entries) >= 4096 {
			return false
		}
	}
	b := l.entries[key]
	if b.started.IsZero() || now.Sub(b.started) >= l.window {
		b = bucket{started: now}
	}
	if b.count >= l.limit {
		l.entries[key] = b
		return false
	}
	b.count++
	l.entries[key] = b
	return true
}
