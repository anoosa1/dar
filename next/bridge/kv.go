package main

import (
	"context"
	"slices"
	"strings"
	"sync"
	"time"
)

// entry is one stored key. Lazy keys are fetched on demand instead of being
// loaded at start; keys with ExpiresAt (Unix ms) are removed by the database.
type entry struct {
	Key       string `json:"key"`
	Value     []byte `json:"value"`
	Lazy      bool   `json:"lazy,omitempty"`
	ExpiresAt int64  `json:"expiresAt,omitempty"`
}

// backend is where an account's keys live: the runtime, which encrypts them
// and keeps them in SpacetimeDB. Commits are atomic.
type backend interface {
	load(ctx context.Context, account, cursor string) ([]entry, string, error)
	get(ctx context.Context, account string, keys []string) (map[string][]byte, error)
	commit(ctx context.Context, account string, puts []entry, deletes []string) error
}

// Namespaces that grow with every message or app state change stay in the
// database and are read when needed; everything else is held in memory.
var lazyPrefixes = []string{"mac/", "secret/", "evbuf/", "out/"}

func isLazy(key string) bool {
	for _, prefix := range lazyPrefixes {
		if strings.HasPrefix(key, prefix) {
			return true
		}
	}
	return false
}

const lazyCacheLimit = 20000

type kv struct {
	account string
	backend backend

	mu       sync.RWMutex
	cache    map[string][]byte
	lazy     map[string][]byte // fetched lazy keys; nil means known to be missing
	commitMu sync.Mutex
}

type txn struct {
	mu      sync.Mutex
	puts    map[string]entry
	deletes map[string]bool
}

type txnKey struct{}

func newKV(account string, b backend) *kv {
	return &kv{account: account, backend: b, cache: map[string][]byte{}, lazy: map[string][]byte{}}
}

// open loads every non-lazy key for the account.
func (s *kv) open(ctx context.Context) error {
	cursor := ""
	for {
		entries, next, err := s.backend.load(ctx, s.account, cursor)
		if err != nil {
			return err
		}
		s.mu.Lock()
		for _, e := range entries {
			s.cache[e.Key] = e.Value
		}
		s.mu.Unlock()
		if next == "" {
			return nil
		}
		cursor = next
	}
}

func currentTxn(ctx context.Context) *txn {
	t, _ := ctx.Value(txnKey{}).(*txn)
	return t
}

func (s *kv) get(ctx context.Context, key string) ([]byte, bool, error) {
	values, err := s.getMany(ctx, []string{key})
	if err != nil {
		return nil, false, err
	}
	value, ok := values[key]
	return value, ok, nil
}

// getMany returns the keys that exist; missing keys are absent from the map.
func (s *kv) getMany(ctx context.Context, keys []string) (map[string][]byte, error) {
	result := make(map[string][]byte, len(keys))
	var fetch []string
	t := currentTxn(ctx)
	if t != nil {
		t.mu.Lock()
	}
	s.mu.RLock()
	for _, key := range keys {
		if t != nil {
			if e, ok := t.puts[key]; ok {
				result[key] = e.Value
				continue
			}
			if t.deletes[key] {
				continue
			}
		}
		if !isLazy(key) {
			if value, ok := s.cache[key]; ok {
				result[key] = value
			}
			continue
		}
		if value, ok := s.lazy[key]; ok {
			if value != nil {
				result[key] = value
			}
			continue
		}
		fetch = append(fetch, key)
	}
	s.mu.RUnlock()
	if t != nil {
		t.mu.Unlock()
	}
	if len(fetch) == 0 {
		return result, nil
	}
	found, err := s.backend.get(ctx, s.account, fetch)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	if len(s.lazy) > lazyCacheLimit {
		s.lazy = map[string][]byte{}
	}
	for _, key := range fetch {
		value, ok := found[key]
		if ok {
			result[key] = value
		}
		s.lazy[key] = value // nil remembers a missing key
	}
	s.mu.Unlock()
	return result, nil
}

// scan lists the in-memory keys under a prefix, including uncommitted changes in a transaction.
func (s *kv) scan(ctx context.Context, prefix string) map[string][]byte {
	result := map[string][]byte{}
	s.mu.RLock()
	for key, value := range s.cache {
		if strings.HasPrefix(key, prefix) {
			result[key] = value
		}
	}
	s.mu.RUnlock()
	if t := currentTxn(ctx); t != nil {
		t.mu.Lock()
		for key, e := range t.puts {
			if strings.HasPrefix(key, prefix) {
				result[key] = e.Value
			}
		}
		for key := range t.deletes {
			if strings.HasPrefix(key, prefix) {
				delete(result, key)
			}
		}
		t.mu.Unlock()
	}
	return result
}

func expiry(ttl time.Duration) int64 {
	if ttl <= 0 {
		return 0
	}
	return time.Now().Add(ttl).UnixMilli()
}

func (s *kv) put(ctx context.Context, key string, value []byte, ttl time.Duration) error {
	return s.write(ctx, []entry{{Key: key, Value: value, Lazy: isLazy(key), ExpiresAt: expiry(ttl)}}, nil)
}

func (s *kv) delete(ctx context.Context, keys ...string) error {
	return s.write(ctx, nil, keys)
}

// write records changes in the current transaction, or commits them now.
func (s *kv) write(ctx context.Context, puts []entry, deletes []string) error {
	if len(puts) == 0 && len(deletes) == 0 {
		return nil
	}
	if t := currentTxn(ctx); t != nil {
		t.mu.Lock()
		defer t.mu.Unlock()
		for _, key := range deletes {
			delete(t.puts, key)
			t.deletes[key] = true
		}
		for _, e := range puts {
			delete(t.deletes, e.Key)
			t.puts[e.Key] = e
		}
		return nil
	}
	return s.commit(ctx, puts, deletes)
}

// transaction runs fn so its writes reach the database together or not at all.
func (s *kv) transaction(ctx context.Context, fn func(context.Context) error) error {
	if currentTxn(ctx) != nil {
		return fn(ctx)
	}
	t := &txn{puts: map[string]entry{}, deletes: map[string]bool{}}
	if err := fn(context.WithValue(ctx, txnKey{}, t)); err != nil {
		return err
	}
	puts := make([]entry, 0, len(t.puts))
	for _, e := range t.puts {
		puts = append(puts, e)
	}
	deletes := make([]string, 0, len(t.deletes))
	for key := range t.deletes {
		deletes = append(deletes, key)
	}
	return s.commit(ctx, puts, deletes)
}

func (s *kv) commit(ctx context.Context, puts []entry, deletes []string) error {
	if len(puts) == 0 && len(deletes) == 0 {
		return nil
	}
	slices.SortFunc(puts, func(a, b entry) int { return strings.Compare(a.Key, b.Key) })
	slices.Sort(deletes)
	s.commitMu.Lock()
	defer s.commitMu.Unlock()
	if err := s.backend.commit(ctx, s.account, puts, deletes); err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, key := range deletes {
		delete(s.cache, key)
		if isLazy(key) {
			s.lazy[key] = nil
		}
	}
	for _, e := range puts {
		if e.Lazy {
			s.lazy[e.Key] = e.Value
		} else {
			s.cache[e.Key] = e.Value
		}
	}
	return nil
}

// forget drops everything held in memory, after the account's keys were deleted.
func (s *kv) forget() {
	s.mu.Lock()
	s.cache = map[string][]byte{}
	s.lazy = map[string][]byte{}
	s.mu.Unlock()
}
