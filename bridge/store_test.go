package main

import (
	"context"
	"errors"
	"strings"
	"sync"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/types"
	waLog "go.mau.fi/whatsmeow/util/log"
)

// memoryBackend stands in for the runtime and counts round trips.
type memoryBackend struct {
	mu      sync.Mutex
	rows    map[string]entry
	commits int
	gets    int
	failing bool
}

func newMemory() *memoryBackend { return &memoryBackend{rows: map[string]entry{}} }

func (m *memoryBackend) load(_ context.Context, account, cursor string) ([]entry, string, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	var result []entry
	for name, e := range m.rows {
		if !e.Lazy && strings.HasPrefix(name, account+"|") {
			result = append(result, e)
		}
	}
	return result, "", nil
}

func (m *memoryBackend) get(_ context.Context, account string, keys []string) (map[string][]byte, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.gets++
	result := map[string][]byte{}
	for _, key := range keys {
		if e, ok := m.rows[account+"|"+key]; ok {
			result[key] = e.Value
		}
	}
	return result, nil
}

func (m *memoryBackend) commit(_ context.Context, account string, puts []entry, deletes []string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.failing {
		return errors.New("database unavailable")
	}
	m.commits++
	for _, key := range deletes {
		delete(m.rows, account+"|"+key)
	}
	for _, e := range puts {
		m.rows[account+"|"+e.Key] = e
	}
	return nil
}

func testStore(t *testing.T) (*waStore, *memoryBackend) {
	t.Helper()
	m := newMemory()
	return newWAStore(newKV("acct-0001", m), waLog.Noop), m
}

func TestTransactionIsAtomicAndVisibleInside(t *testing.T) {
	s, m := testStore(t)
	ctx := context.Background()
	err := s.DoDecryptionTxn(ctx, func(ctx context.Context) error {
		if err := s.PutSession(ctx, "123.0:1", []byte("session")); err != nil {
			return err
		}
		got, err := s.GetSession(ctx, "123.0:1")
		if err != nil || string(got) != "session" {
			t.Fatalf("uncommitted write not visible inside the transaction: %q %v", got, err)
		}
		return s.PutBufferedEvent(ctx, [32]byte{1}, []byte("plain"), time.Unix(100, 0))
	})
	if err != nil || m.commits != 1 {
		t.Fatalf("want one commit, got %d (%v)", m.commits, err)
	}
	if buffered := m.rows["acct-0001|evbuf/01"+strings.Repeat("0", 62)]; !buffered.Lazy || buffered.ExpiresAt == 0 {
		t.Fatal("buffered events are lazy and expire")
	}
	failed := errors.New("decrypt failed")
	err = s.DoDecryptionTxn(ctx, func(ctx context.Context) error {
		_ = s.PutSession(ctx, "456.0:1", []byte("discarded"))
		return failed
	})
	if !errors.Is(err, failed) || m.commits != 1 {
		t.Fatal("a failed transaction must not write")
	}
	if has, _ := s.HasSession(ctx, "456.0:1"); has {
		t.Fatal("a failed transaction must not change memory")
	}
}

func TestFailedCommitLeavesMemoryUnchanged(t *testing.T) {
	s, m := testStore(t)
	ctx := context.Background()
	m.failing = true
	if err := s.PutIdentity(ctx, "1.0:1", [32]byte{9}); err == nil {
		t.Fatal("expected the write to fail")
	}
	if trusted, _ := s.IsTrustedIdentity(ctx, "1.0:1", [32]byte{1}); !trusted {
		t.Fatal("unknown identities are trusted; the failed write must not be cached")
	}
}

func TestLazyKeysAreFetchedOnceAndRemembered(t *testing.T) {
	s, m := testStore(t)
	ctx := context.Background()
	if mac, _ := s.GetAppStateMutationMAC(ctx, "regular", []byte{1}); mac != nil {
		t.Fatal("missing MAC")
	}
	if mac, _ := s.GetAppStateMutationMAC(ctx, "regular", []byte{1}); mac != nil || m.gets != 1 {
		t.Fatalf("a missing lazy key is remembered, got %d gets", m.gets)
	}
	_ = s.PutAppStateMutationMACs(ctx, "regular", 2, []store.AppStateMutationMAC{{IndexMAC: []byte{1}, ValueMAC: []byte{7}}, {IndexMAC: []byte{2}, ValueMAC: []byte{8}}})
	if mac, _ := s.GetAppStateMutationMAC(ctx, "regular", []byte{1}); string(mac) != "\x07" || m.gets != 1 {
		t.Fatal("written lazy keys are served from memory")
	}
	// A fresh process reads them from the database.
	fresh := newWAStore(newKV("acct-0001", m), waLog.Noop)
	if mac, _ := fresh.GetAppStateMutationMAC(ctx, "regular", []byte{2}); string(mac) != "\x08" {
		t.Fatal("lazy keys persist")
	}
}

func TestPreKeys(t *testing.T) {
	s, _ := testStore(t)
	ctx := context.Background()
	keys, err := s.GetOrGenPreKeys(ctx, 5)
	if err != nil || len(keys) != 5 || keys[0].KeyID != 1 || keys[4].KeyID != 5 {
		t.Fatalf("generate: %v %d", err, len(keys))
	}
	again, _ := s.GetOrGenPreKeys(ctx, 3)
	if again[0].KeyID != 1 || *again[0].Priv != *keys[0].Priv {
		t.Fatal("unuploaded keys are returned before new ones")
	}
	_ = s.MarkPreKeysAsUploaded(ctx, 3)
	if count, _ := s.UploadedPreKeyCount(ctx); count != 3 {
		t.Fatalf("uploaded count %d", count)
	}
	next, _ := s.GetOrGenPreKeys(ctx, 4)
	if next[0].KeyID != 4 || next[2].KeyID != 6 || next[3].KeyID != 7 {
		t.Fatalf("unexpected IDs %d..%d", next[0].KeyID, next[3].KeyID)
	}
	one, _ := s.GenOnePreKey(ctx)
	if one.KeyID != 8 {
		t.Fatalf("one key %d", one.KeyID)
	}
	_ = s.RemovePreKey(ctx, 2)
	if gone, _ := s.GetPreKey(ctx, 2); gone != nil {
		t.Fatal("removed")
	}
	if got, _ := s.GetPreKey(ctx, 1); got == nil || *got.Priv != *keys[0].Priv {
		t.Fatal("stored key")
	}
}

func TestMigratePNToLID(t *testing.T) {
	s, m := testStore(t)
	ctx := context.Background()
	pn, lid := types.NewJID("15550001", types.DefaultUserServer), types.NewJID("98765", types.HiddenUserServer)
	_ = s.PutSession(ctx, pn.SignalAddressUser()+":0", []byte("a"))
	_ = s.PutSession(ctx, pn.SignalAddressUser()+":3", []byte("b"))
	_ = s.PutSession(ctx, "15550001999:0", []byte("other user"))
	_ = s.PutIdentity(ctx, pn.SignalAddressUser()+":0", [32]byte{4})
	_ = s.PutSenderKey(ctx, "group@g.us", pn.SignalAddressUser()+":3", []byte("sk"))
	before := m.commits
	if err := s.MigratePNToLID(ctx, pn, lid); err != nil {
		t.Fatal(err)
	}
	if m.commits != before+1 {
		t.Fatal("migration is one transaction")
	}
	if got, _ := s.GetSession(ctx, lid.SignalAddressUser()+":3"); string(got) != "b" {
		t.Fatal("session moved")
	}
	if has, _ := s.HasSession(ctx, pn.SignalAddressUser()+":0"); has {
		t.Fatal("old session removed")
	}
	if has, _ := s.HasSession(ctx, "15550001999:0"); !has {
		t.Fatal("a longer number with the same prefix is another user")
	}
	if trusted, _ := s.IsTrustedIdentity(ctx, lid.SignalAddressUser()+":0", [32]byte{4}); !trusted {
		t.Fatal("identity moved")
	}
	if key, _ := s.GetSenderKey(ctx, "group@g.us", lid.SignalAddressUser()+":3"); string(key) != "sk" {
		t.Fatal("sender key moved")
	}
}

func TestLIDMapAndMessageSecrets(t *testing.T) {
	s, _ := testStore(t)
	ctx := context.Background()
	pn, lid := types.NewJID("15550002", types.DefaultUserServer), types.NewJID("111", types.HiddenUserServer)
	_ = s.PutLIDMapping(ctx, lid, pn)
	if got, _ := s.GetLIDForPN(ctx, types.NewADJID("15550002", 0, 4)); got.User != "111" || got.Device != 4 || got.Server != types.HiddenUserServer {
		t.Fatalf("lid for pn: %v", got)
	}
	newLID := types.NewJID("222", types.HiddenUserServer)
	_ = s.PutManyLIDMappings(ctx, []store.LIDMapping{{LID: newLID, PN: pn}})
	if old, _ := s.GetPNForLID(ctx, lid); !old.IsEmpty() {
		t.Fatal("a replaced LID no longer maps to the number")
	}
	chat := types.NewJID("group", types.GroupServer)
	_ = s.PutMessageSecret(ctx, chat, newLID, "MSG1", []byte("secret"))
	secret, sender, err := s.GetMessageSecret(ctx, chat, pn, "MSG1")
	if err != nil || string(secret) != "secret" || sender != newLID {
		t.Fatalf("secret by alternate sender: %q %v %v", secret, sender, err)
	}
}

func TestContactsAndChatSettings(t *testing.T) {
	s, _ := testStore(t)
	ctx := context.Background()
	jid := types.NewJID("15550003", types.DefaultUserServer)
	changed, previous, _ := s.PutPushName(ctx, jid, "Sara")
	if !changed || previous != "" {
		t.Fatal("first push name")
	}
	if changed, _, _ = s.PutPushName(ctx, jid, "Sara"); changed {
		t.Fatal("same push name is not a change")
	}
	_ = s.PutAllContactNames(ctx, []store.ContactEntry{{JID: jid, FirstName: "S", FullName: "Old"}, {JID: jid, FirstName: "Sara", FullName: "Sara Haddad"}})
	info, _ := s.GetContact(ctx, jid)
	if !info.Found || info.FullName != "Sara Haddad" || info.PushName != "Sara" {
		t.Fatalf("contact: %+v", info)
	}
	all, _ := s.GetAllContacts(ctx)
	if len(all) != 1 || all[jid].FirstName != "Sara" {
		t.Fatal("all contacts")
	}
	_ = s.PutMutedUntil(ctx, jid, store.MutedForever)
	_ = s.PutArchived(ctx, jid, true)
	settings, _ := s.GetChatSettings(ctx, jid)
	if !settings.Found || !settings.Archived || settings.MutedUntil != store.MutedForever {
		t.Fatalf("settings: %+v", settings)
	}
}

func TestAppStateKeysAndVersions(t *testing.T) {
	s, _ := testStore(t)
	ctx := context.Background()
	_ = s.PutAppStateSyncKey(ctx, []byte{1}, store.AppStateSyncKey{Data: []byte("a"), Timestamp: 10})
	_ = s.PutAppStateSyncKey(ctx, []byte{2}, store.AppStateSyncKey{Data: []byte("b"), Timestamp: 20})
	_ = s.PutAppStateSyncKey(ctx, []byte{1}, store.AppStateSyncKey{Data: []byte("older"), Timestamp: 5})
	if latest, _ := s.GetLatestAppStateSyncKeyID(ctx); string(latest) != "\x02" {
		t.Fatal("latest key")
	}
	if key, _ := s.GetAppStateSyncKey(ctx, []byte{1}); string(key.Data) != "a" {
		t.Fatal("an older key does not replace a newer one")
	}
	if version, _, err := s.GetAppStateVersion(ctx, "regular"); version != 0 || err != nil {
		t.Fatal("missing version is 0")
	}
	var hash [128]byte
	hash[0] = 3
	_ = s.PutAppStateVersion(ctx, "regular", 7, hash)
	if version, got, _ := s.GetAppStateVersion(ctx, "regular"); version != 7 || got != hash {
		t.Fatal("version")
	}
}

func TestDeviceRoundTrip(t *testing.T) {
	m := newMemory()
	c := newContainer(newKV("acct-0002", m), waLog.Noop)
	device := c.newDevice()
	jid := types.NewADJID("15550004", 0, 12)
	device.ID, device.PushName = &jid, "Anas"
	if err := c.PutDevice(context.Background(), device); err != nil {
		t.Fatal(err)
	}
	reopened := newKV("acct-0002", m)
	if err := reopened.open(context.Background()); err != nil {
		t.Fatal(err)
	}
	loaded, err := newContainer(reopened, waLog.Noop).loadDevice(context.Background())
	if err != nil || loaded == nil {
		t.Fatalf("load: %v", err)
	}
	if *loaded.ID != jid || *loaded.IdentityKey.Priv != *device.IdentityKey.Priv || *loaded.NoiseKey.Pub != *device.NoiseKey.Pub ||
		*loaded.SignedPreKey.Signature != *device.SignedPreKey.Signature || loaded.RegistrationID != device.RegistrationID || !loaded.Initialized {
		t.Fatal("device keys must survive a restart")
	}
	if missing, _ := newContainer(newKV("acct-0003", m), waLog.Noop).loadDevice(context.Background()); missing != nil {
		t.Fatal("an unlinked account has no device")
	}
}
