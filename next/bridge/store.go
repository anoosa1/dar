package main

// A whatsmeow store kept in Dar's database instead of SQLite or Postgres.
// Behaviour follows whatsmeow's own sqlstore (MPL-2.0) method by method; the
// runtime encrypts every value before SpacetimeDB stores it.

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/util/keys"
	waLog "go.mau.fi/whatsmeow/util/log"
)

var errInvalidLength = errors.New("stored key has an invalid length")

type waStore struct {
	kv  *kv
	log waLog.Logger

	preKeyMu  sync.Mutex
	contactMu sync.Mutex
	lidMu     sync.Mutex
	migrated  sync.Map
}

var (
	_ store.AllSessionSpecificStores = (*waStore)(nil)
	_ store.LIDStore                 = (*waStore)(nil)
)

func newWAStore(s *kv, log waLog.Logger) *waStore { return &waStore{kv: s, log: log} }

func getJSON[T any](ctx context.Context, s *kv, key string) (T, bool, error) {
	var value T
	raw, ok, err := s.get(ctx, key)
	if err != nil || !ok {
		return value, false, err
	}
	if err = json.Unmarshal(raw, &value); err != nil {
		return value, false, fmt.Errorf("decode %s: %w", strings.SplitN(key, "/", 2)[0], err)
	}
	return value, true, nil
}

func putJSON(ctx context.Context, s *kv, key string, value any, ttl time.Duration) error {
	raw, err := json.Marshal(value)
	if err != nil {
		return err
	}
	return s.put(ctx, key, raw, ttl)
}

func (s *waStore) deletePrefix(ctx context.Context, prefix string) error {
	found := s.kv.scan(ctx, prefix)
	keys := make([]string, 0, len(found))
	for key := range found {
		keys = append(keys, key)
	}
	return s.kv.delete(ctx, keys...)
}

// Identities

func (s *waStore) PutIdentity(ctx context.Context, address string, key [32]byte) error {
	return s.kv.put(ctx, "identity/"+address, key[:], 0)
}

func (s *waStore) DeleteAllIdentities(ctx context.Context, phone string) error {
	return s.deletePrefix(ctx, "identity/"+phone+":")
}

func (s *waStore) DeleteIdentity(ctx context.Context, address string) error {
	return s.kv.delete(ctx, "identity/"+address)
}

func (s *waStore) IsTrustedIdentity(ctx context.Context, address string, key [32]byte) (bool, error) {
	existing, ok, err := s.kv.get(ctx, "identity/"+address)
	if err != nil {
		return false, err
	} else if !ok {
		// Trust if not known; it is saved automatically later.
		return true, nil
	} else if len(existing) != 32 {
		return false, errInvalidLength
	}
	return [32]byte(existing) == key, nil
}

// Sessions

func (s *waStore) GetSession(ctx context.Context, address string) ([]byte, error) {
	value, _, err := s.kv.get(ctx, "session/"+address)
	return value, err
}

func (s *waStore) HasSession(ctx context.Context, address string) (bool, error) {
	_, ok, err := s.kv.get(ctx, "session/"+address)
	return ok, err
}

func (s *waStore) GetManySessions(ctx context.Context, addresses []string) (map[string][]byte, error) {
	if len(addresses) == 0 {
		return nil, nil
	}
	keys := make([]string, len(addresses))
	for i, address := range addresses {
		keys[i] = "session/" + address
	}
	found, err := s.kv.getMany(ctx, keys)
	if err != nil {
		return nil, err
	}
	result := make(map[string][]byte, len(addresses))
	for _, address := range addresses {
		result[address] = found["session/"+address]
	}
	return result, nil
}

func (s *waStore) PutSession(ctx context.Context, address string, session []byte) error {
	return s.kv.put(ctx, "session/"+address, session, 0)
}

func (s *waStore) PutManySessions(ctx context.Context, sessions map[string][]byte) error {
	return s.kv.transaction(ctx, func(ctx context.Context) error {
		for address, session := range sessions {
			if err := s.PutSession(ctx, address, session); err != nil {
				return err
			}
		}
		return nil
	})
}

func (s *waStore) DeleteAllSessions(ctx context.Context, phone string) error {
	return s.deletePrefix(ctx, "session/"+phone+":")
}

func (s *waStore) DeleteSession(ctx context.Context, address string) error {
	return s.kv.delete(ctx, "session/"+address)
}

// moveKeys copies keys whose address starts with "<from>:" to the same address
// with every occurrence of from replaced by to, then deletes the originals.
func (s *waStore) moveKeys(ctx context.Context, prefix, from, to string, address func(rest string) (string, func(string) string)) int {
	moved := 0
	for key, value := range s.kv.scan(ctx, prefix) {
		addr, rebuild := address(strings.TrimPrefix(key, prefix))
		if !strings.HasPrefix(addr, from+":") {
			continue
		}
		_ = s.kv.put(ctx, prefix+rebuild(strings.ReplaceAll(addr, from, to)), value, 0)
		_ = s.kv.delete(ctx, key)
		moved++
	}
	return moved
}

func (s *waStore) MigratePNToLID(ctx context.Context, pn, lid types.JID) error {
	pnSignal := pn.SignalAddressUser()
	if _, done := s.migrated.LoadOrStore(pnSignal, true); done {
		return nil
	}
	lidSignal := lid.SignalAddressUser()
	var sessions, identities, senderKeys int
	plain := func(rest string) (string, func(string) string) { return rest, func(a string) string { return a } }
	err := s.kv.transaction(ctx, func(ctx context.Context) error {
		sessions = s.moveKeys(ctx, "session/", pnSignal, lidSignal, plain)
		identities = s.moveKeys(ctx, "identity/", pnSignal, lidSignal, plain)
		senderKeys = s.moveKeys(ctx, "sender/", pnSignal, lidSignal, func(rest string) (string, func(string) string) {
			group, user, _ := strings.Cut(rest, "/")
			return user, func(a string) string { return group + "/" + a }
		})
		return nil
	})
	if err != nil {
		s.migrated.Delete(pnSignal)
		return err
	}
	if sessions > 0 || identities > 0 || senderKeys > 0 {
		s.log.Infof("Migrated %d sessions, %d identity keys and %d sender keys from %s to %s", sessions, identities, senderKeys, pnSignal, lidSignal)
	}
	return nil
}

// Pre-keys

type storedPreKey struct {
	Priv     []byte `json:"priv"`
	Uploaded bool   `json:"uploaded"`
}

func preKeyName(id uint32) string { return fmt.Sprintf("prekey/%010d", id) }

func (s *waStore) preKeys(ctx context.Context) map[uint32]storedPreKey {
	result := map[uint32]storedPreKey{}
	for key, raw := range s.kv.scan(ctx, "prekey/") {
		id, err := strconv.ParseUint(strings.TrimPrefix(key, "prekey/"), 10, 32)
		var value storedPreKey
		if err == nil && json.Unmarshal(raw, &value) == nil {
			result[uint32(id)] = value
		}
	}
	return result
}

func (s *waStore) nextPreKeyID(ctx context.Context) uint32 {
	var last uint32
	for id := range s.preKeys(ctx) {
		last = max(last, id)
	}
	return last + 1
}

func (s *waStore) genOnePreKey(ctx context.Context, id uint32, uploaded bool) (*keys.PreKey, error) {
	key := keys.NewPreKey(id)
	return key, putJSON(ctx, s.kv, preKeyName(id), storedPreKey{Priv: key.Priv[:], Uploaded: uploaded}, 0)
}

func (s *waStore) GenOnePreKey(ctx context.Context) (*keys.PreKey, error) {
	s.preKeyMu.Lock()
	defer s.preKeyMu.Unlock()
	return s.genOnePreKey(ctx, s.nextPreKeyID(ctx), true)
}

func (s *waStore) GetOrGenPreKeys(ctx context.Context, count uint32) ([]*keys.PreKey, error) {
	s.preKeyMu.Lock()
	defer s.preKeyMu.Unlock()
	stored := s.preKeys(ctx)
	ids := make([]uint32, 0, len(stored))
	for id, value := range stored {
		if !value.Uploaded {
			ids = append(ids, id)
		}
	}
	slices.Sort(ids)
	if uint32(len(ids)) > count {
		ids = ids[:count]
	}
	result := make([]*keys.PreKey, 0, count)
	for _, id := range ids {
		priv := stored[id].Priv
		if len(priv) != 32 {
			return nil, errInvalidLength
		}
		result = append(result, &keys.PreKey{KeyPair: *keys.NewKeyPairFromPrivateKey([32]byte(priv)), KeyID: id})
	}
	if uint32(len(result)) < count {
		next := s.nextPreKeyID(ctx)
		err := s.kv.transaction(ctx, func(ctx context.Context) error {
			for uint32(len(result)) < count {
				key, err := s.genOnePreKey(ctx, next, false)
				if err != nil {
					return err
				}
				result = append(result, key)
				next++
			}
			return nil
		})
		if err != nil {
			return nil, fmt.Errorf("failed to generate prekeys: %w", err)
		}
	}
	return result, nil
}

func (s *waStore) GetPreKey(ctx context.Context, id uint32) (*keys.PreKey, error) {
	value, ok, err := getJSON[storedPreKey](ctx, s.kv, preKeyName(id))
	if err != nil || !ok {
		return nil, err
	}
	if len(value.Priv) != 32 {
		return nil, errInvalidLength
	}
	return &keys.PreKey{KeyPair: *keys.NewKeyPairFromPrivateKey([32]byte(value.Priv)), KeyID: id}, nil
}

func (s *waStore) RemovePreKey(ctx context.Context, id uint32) error {
	return s.kv.delete(ctx, preKeyName(id))
}

func (s *waStore) MarkPreKeysAsUploaded(ctx context.Context, upToID uint32) error {
	return s.kv.transaction(ctx, func(ctx context.Context) error {
		for id, value := range s.preKeys(ctx) {
			if id <= upToID && !value.Uploaded {
				value.Uploaded = true
				if err := putJSON(ctx, s.kv, preKeyName(id), value, 0); err != nil {
					return err
				}
			}
		}
		return nil
	})
}

func (s *waStore) UploadedPreKeyCount(ctx context.Context) (int, error) {
	count := 0
	for _, value := range s.preKeys(ctx) {
		if value.Uploaded {
			count++
		}
	}
	return count, nil
}

// Sender keys

func (s *waStore) PutSenderKey(ctx context.Context, group, user string, session []byte) error {
	return s.kv.put(ctx, "sender/"+group+"/"+user, session, 0)
}

func (s *waStore) GetSenderKey(ctx context.Context, group, user string) ([]byte, error) {
	value, _, err := s.kv.get(ctx, "sender/"+group+"/"+user)
	return value, err
}

// App state sync keys

type storedAppStateKey struct {
	Data        []byte `json:"data"`
	Fingerprint []byte `json:"fingerprint"`
	Timestamp   int64  `json:"timestamp"`
}

func (s *waStore) PutAppStateSyncKey(ctx context.Context, id []byte, key store.AppStateSyncKey) error {
	name := "appkey/" + hex.EncodeToString(id)
	existing, ok, err := getJSON[storedAppStateKey](ctx, s.kv, name)
	if err != nil {
		return err
	} else if ok && key.Timestamp <= existing.Timestamp {
		return nil
	}
	return putJSON(ctx, s.kv, name, storedAppStateKey(key), 0)
}

type appStateKeyWithID struct {
	id  []byte
	key storedAppStateKey
}

// appStateKeys lists the keys newest first.
func (s *waStore) appStateKeys(ctx context.Context) []appStateKeyWithID {
	var result []appStateKeyWithID
	for name, raw := range s.kv.scan(ctx, "appkey/") {
		id, err := hex.DecodeString(strings.TrimPrefix(name, "appkey/"))
		var value storedAppStateKey
		if err == nil && json.Unmarshal(raw, &value) == nil {
			result = append(result, appStateKeyWithID{id, value})
		}
	}
	slices.SortFunc(result, func(a, b appStateKeyWithID) int { return int(b.key.Timestamp - a.key.Timestamp) })
	return result
}

func (s *waStore) GetAllAppStateSyncKeys(ctx context.Context) ([]*store.AppStateSyncKey, error) {
	var result []*store.AppStateSyncKey
	for _, item := range s.appStateKeys(ctx) {
		key := store.AppStateSyncKey(item.key)
		result = append(result, &key)
	}
	return result, nil
}

func (s *waStore) GetAppStateSyncKey(ctx context.Context, id []byte) (*store.AppStateSyncKey, error) {
	value, ok, err := getJSON[storedAppStateKey](ctx, s.kv, "appkey/"+hex.EncodeToString(id))
	if err != nil || !ok {
		return nil, err
	}
	key := store.AppStateSyncKey(value)
	return &key, nil
}

func (s *waStore) GetLatestAppStateSyncKeyID(ctx context.Context) ([]byte, error) {
	all := s.appStateKeys(ctx)
	if len(all) == 0 {
		return nil, nil
	}
	return all[0].id, nil
}

// App state versions and mutation MACs

type storedAppStateVersion struct {
	Version uint64 `json:"version"`
	Hash    []byte `json:"hash"`
}

func (s *waStore) PutAppStateVersion(ctx context.Context, name string, version uint64, hash [128]byte) error {
	return putJSON(ctx, s.kv, "appver/"+name, storedAppStateVersion{version, hash[:]}, 0)
}

func (s *waStore) GetAppStateVersion(ctx context.Context, name string) (version uint64, hash [128]byte, err error) {
	value, ok, err := getJSON[storedAppStateVersion](ctx, s.kv, "appver/"+name)
	if err != nil || !ok {
		return 0, hash, err
	} else if len(value.Hash) != 128 {
		return 0, hash, errInvalidLength
	} else if value.Version == 0 {
		return 0, hash, fmt.Errorf("invalid saved app state version 0 for name %s", name)
	}
	return value.Version, [128]byte(value.Hash), nil
}

func (s *waStore) DeleteAppStateVersion(ctx context.Context, name string) error {
	return s.kv.delete(ctx, "appver/"+name)
}

func macName(name string, indexMAC []byte) string {
	return "mac/" + name + "/" + hex.EncodeToString(indexMAC)
}

func (s *waStore) PutAppStateMutationMACs(ctx context.Context, name string, version uint64, mutations []store.AppStateMutationMAC) error {
	if len(mutations) == 0 {
		return nil
	}
	return s.kv.transaction(ctx, func(ctx context.Context) error {
		for _, mutation := range mutations {
			if err := s.kv.put(ctx, macName(name, mutation.IndexMAC), mutation.ValueMAC, 0); err != nil {
				return err
			}
		}
		return nil
	})
}

func (s *waStore) DeleteAppStateMutationMACs(ctx context.Context, name string, indexMACs [][]byte) error {
	keys := make([]string, len(indexMACs))
	for i, indexMAC := range indexMACs {
		keys[i] = macName(name, indexMAC)
	}
	return s.kv.delete(ctx, keys...)
}

func (s *waStore) GetAppStateMutationMAC(ctx context.Context, name string, indexMAC []byte) ([]byte, error) {
	value, _, err := s.kv.get(ctx, macName(name, indexMAC))
	return value, err
}

// Contacts

type storedContact struct {
	FirstName     string `json:"firstName,omitempty"`
	FullName      string `json:"fullName,omitempty"`
	PushName      string `json:"pushName,omitempty"`
	BusinessName  string `json:"businessName,omitempty"`
	RedactedPhone string `json:"redactedPhone,omitempty"`
}

func contactName(user types.JID) string { return "contact/" + user.String() }

func (s *waStore) PutPushName(ctx context.Context, user types.JID, pushName string) (bool, string, error) {
	s.contactMu.Lock()
	defer s.contactMu.Unlock()
	contact, _, err := getJSON[storedContact](ctx, s.kv, contactName(user))
	if err != nil || contact.PushName == pushName {
		return false, "", err
	}
	previous := contact.PushName
	contact.PushName = pushName
	if err = putJSON(ctx, s.kv, contactName(user), contact, 0); err != nil {
		return false, "", err
	}
	return true, previous, nil
}

func (s *waStore) PutBusinessName(ctx context.Context, user types.JID, businessName string) (bool, string, error) {
	s.contactMu.Lock()
	defer s.contactMu.Unlock()
	contact, _, err := getJSON[storedContact](ctx, s.kv, contactName(user))
	if err != nil || contact.BusinessName == businessName {
		return false, "", err
	}
	previous := contact.BusinessName
	contact.BusinessName = businessName
	if err = putJSON(ctx, s.kv, contactName(user), contact, 0); err != nil {
		return false, "", err
	}
	return true, previous, nil
}

func (s *waStore) PutContactName(ctx context.Context, user types.JID, firstName, fullName string) error {
	s.contactMu.Lock()
	defer s.contactMu.Unlock()
	contact, _, err := getJSON[storedContact](ctx, s.kv, contactName(user))
	if err != nil || contact.FirstName == firstName && contact.FullName == fullName {
		return err
	}
	contact.FirstName, contact.FullName = firstName, fullName
	return putJSON(ctx, s.kv, contactName(user), contact, 0)
}

func (s *waStore) PutAllContactNames(ctx context.Context, contacts []store.ContactEntry) error {
	s.contactMu.Lock()
	defer s.contactMu.Unlock()
	// The last entry for a contact is the newest.
	latest := map[types.JID]store.ContactEntry{}
	for _, entry := range contacts {
		latest[entry.JID] = entry
	}
	return s.kv.transaction(ctx, func(ctx context.Context) error {
		for user, entry := range latest {
			contact, _, err := getJSON[storedContact](ctx, s.kv, contactName(user))
			if err != nil {
				return err
			}
			if contact.FirstName == entry.FirstName && contact.FullName == entry.FullName {
				continue
			}
			contact.FirstName, contact.FullName = entry.FirstName, entry.FullName
			if err = putJSON(ctx, s.kv, contactName(user), contact, 0); err != nil {
				return err
			}
		}
		return nil
	})
}

func (s *waStore) PutManyRedactedPhones(ctx context.Context, entries []store.RedactedPhoneEntry) error {
	s.contactMu.Lock()
	defer s.contactMu.Unlock()
	return s.kv.transaction(ctx, func(ctx context.Context) error {
		for _, entry := range entries {
			contact, _, err := getJSON[storedContact](ctx, s.kv, contactName(entry.JID))
			if err != nil {
				return err
			}
			if contact.RedactedPhone == entry.RedactedPhone {
				continue
			}
			contact.RedactedPhone = entry.RedactedPhone
			if err = putJSON(ctx, s.kv, contactName(entry.JID), contact, 0); err != nil {
				return err
			}
		}
		return nil
	})
}

func (c storedContact) info(found bool) types.ContactInfo {
	return types.ContactInfo{Found: found, FirstName: c.FirstName, FullName: c.FullName, PushName: c.PushName, BusinessName: c.BusinessName, RedactedPhone: c.RedactedPhone}
}

func (s *waStore) GetContact(ctx context.Context, user types.JID) (types.ContactInfo, error) {
	contact, found, err := getJSON[storedContact](ctx, s.kv, contactName(user))
	return contact.info(found), err
}

func (s *waStore) GetAllContacts(ctx context.Context) (map[types.JID]types.ContactInfo, error) {
	result := map[types.JID]types.ContactInfo{}
	for key, raw := range s.kv.scan(ctx, "contact/") {
		jid, err := types.ParseJID(strings.TrimPrefix(key, "contact/"))
		var contact storedContact
		if err == nil && json.Unmarshal(raw, &contact) == nil {
			result[jid] = contact.info(true)
		}
	}
	return result, nil
}

// Chat settings

type storedChatSettings struct {
	MutedUntil int64  `json:"mutedUntil,omitempty"`
	Pinned     bool   `json:"pinned,omitempty"`
	Archived   bool   `json:"archived,omitempty"`
	WASA       string `json:"wasa,omitempty"`
}

func (s *waStore) updateChatSettings(ctx context.Context, chat types.JID, change func(*storedChatSettings)) error {
	name := "chatset/" + chat.String()
	value, _, err := getJSON[storedChatSettings](ctx, s.kv, name)
	if err != nil {
		return err
	}
	change(&value)
	return putJSON(ctx, s.kv, name, value, 0)
}

func (s *waStore) PutMutedUntil(ctx context.Context, chat types.JID, mutedUntil time.Time) error {
	var value int64
	if mutedUntil == store.MutedForever {
		value = -1
	} else if !mutedUntil.IsZero() {
		value = mutedUntil.Unix()
	}
	return s.updateChatSettings(ctx, chat, func(c *storedChatSettings) { c.MutedUntil = value })
}

func (s *waStore) PutPinned(ctx context.Context, chat types.JID, pinned bool) error {
	return s.updateChatSettings(ctx, chat, func(c *storedChatSettings) { c.Pinned = pinned })
}

func (s *waStore) PutArchived(ctx context.Context, chat types.JID, archived bool) error {
	return s.updateChatSettings(ctx, chat, func(c *storedChatSettings) { c.Archived = archived })
}

func (s *waStore) PutWASARootSecretID(ctx context.Context, chat types.JID, id types.MessageID) error {
	return s.updateChatSettings(ctx, chat, func(c *storedChatSettings) { c.WASA = id })
}

func (s *waStore) GetWASARootSecretID(ctx context.Context, chat types.JID) (types.MessageID, error) {
	value, _, err := getJSON[storedChatSettings](ctx, s.kv, "chatset/"+chat.String())
	return value.WASA, err
}

func (s *waStore) GetChatSettings(ctx context.Context, chat types.JID) (settings types.LocalChatSettings, err error) {
	value, found, err := getJSON[storedChatSettings](ctx, s.kv, "chatset/"+chat.String())
	if err != nil {
		return settings, err
	}
	settings.Found, settings.Pinned, settings.Archived = found, value.Pinned, value.Archived
	if value.MutedUntil < 0 {
		settings.MutedUntil = store.MutedForever
	} else if value.MutedUntil > 0 {
		settings.MutedUntil = time.Unix(value.MutedUntil, 0)
	}
	return settings, nil
}

// Message secrets

func secretName(chat, sender types.JID, id types.MessageID) string {
	return "secret/" + chat.ToNonAD().String() + "/" + sender.ToNonAD().String() + "/" + id
}

func (s *waStore) PutMessageSecrets(ctx context.Context, inserts []store.MessageSecretInsert) error {
	return s.kv.transaction(ctx, func(ctx context.Context) error {
		for _, insert := range inserts {
			if insert.Chat.IsEmpty() || insert.Sender.IsEmpty() || insert.ID == "" || len(insert.Secret) == 0 {
				continue
			}
			if err := s.kv.put(ctx, secretName(insert.Chat, insert.Sender, insert.ID), insert.Secret, 0); err != nil {
				return err
			}
		}
		return nil
	})
}

func (s *waStore) PutMessageSecret(ctx context.Context, chat, sender types.JID, id types.MessageID, secret []byte) error {
	return s.kv.put(ctx, secretName(chat, sender, id), secret, 0)
}

// alternate returns the phone-number form of a LID or the LID form of a phone number, if known.
func (s *waStore) alternate(ctx context.Context, jid types.JID) types.JID {
	var alt types.JID
	switch jid.Server {
	case types.HiddenUserServer:
		alt, _ = s.GetPNForLID(ctx, jid)
	case types.DefaultUserServer:
		alt, _ = s.GetLIDForPN(ctx, jid)
	}
	return alt
}

func withAlternate(ctx context.Context, s *waStore, jid types.JID) []types.JID {
	jid = jid.ToNonAD()
	if alt := s.alternate(ctx, jid); !alt.IsEmpty() {
		return []types.JID{jid, alt.ToNonAD()}
	}
	return []types.JID{jid}
}

func (s *waStore) GetMessageSecret(ctx context.Context, chat, sender types.JID, id types.MessageID) ([]byte, types.JID, error) {
	type candidate struct {
		key    string
		sender types.JID
	}
	var candidates []candidate
	for _, c := range withAlternate(ctx, s, chat) {
		for _, from := range withAlternate(ctx, s, sender) {
			candidates = append(candidates, candidate{secretName(c, from, id), from})
		}
	}
	names := make([]string, len(candidates))
	for i, c := range candidates {
		names[i] = c.key
	}
	found, err := s.kv.getMany(ctx, names)
	if err != nil {
		return nil, types.EmptyJID, err
	}
	for _, c := range candidates {
		if secret, ok := found[c.key]; ok {
			return secret, c.sender, nil
		}
	}
	return nil, types.EmptyJID, nil
}

// Privacy tokens

type storedPrivacyToken struct {
	Token           []byte `json:"token"`
	Timestamp       int64  `json:"timestamp"`
	SenderTimestamp int64  `json:"senderTimestamp,omitempty"`
}

func (s *waStore) PutPrivacyTokens(ctx context.Context, tokens ...store.PrivacyToken) error {
	return s.kv.transaction(ctx, func(ctx context.Context) error {
		for _, token := range tokens {
			name := "privacy/" + token.User.ToNonAD().String()
			existing, ok, err := getJSON[storedPrivacyToken](ctx, s.kv, name)
			if err != nil {
				return err
			}
			if ok && token.Timestamp.Unix() < existing.Timestamp {
				continue
			}
			value := storedPrivacyToken{Token: token.Token, Timestamp: token.Timestamp.Unix(), SenderTimestamp: existing.SenderTimestamp}
			if !token.SenderTimestamp.IsZero() {
				value.SenderTimestamp = token.SenderTimestamp.Unix()
			}
			if err = putJSON(ctx, s.kv, name, value, 0); err != nil {
				return err
			}
		}
		return nil
	})
}

func (s *waStore) GetPrivacyToken(ctx context.Context, user types.JID) (*store.PrivacyToken, error) {
	var best *store.PrivacyToken
	for _, jid := range withAlternate(ctx, s, user) {
		value, ok, err := getJSON[storedPrivacyToken](ctx, s.kv, "privacy/"+jid.String())
		if err != nil {
			return nil, err
		}
		if !ok || best != nil && value.Timestamp <= best.Timestamp.Unix() {
			continue
		}
		best = &store.PrivacyToken{User: user.ToNonAD(), Token: value.Token, Timestamp: time.Unix(value.Timestamp, 0)}
		if value.SenderTimestamp != 0 {
			best.SenderTimestamp = time.Unix(value.SenderTimestamp, 0)
		}
	}
	return best, nil
}

func (s *waStore) DeleteExpiredPrivacyTokens(ctx context.Context, cutoff time.Time) (int64, error) {
	var expired []string
	for key, raw := range s.kv.scan(ctx, "privacy/") {
		var value storedPrivacyToken
		if json.Unmarshal(raw, &value) == nil && value.Timestamp < cutoff.Unix() {
			expired = append(expired, key)
		}
	}
	return int64(len(expired)), s.kv.delete(ctx, expired...)
}

// NCT salt

func (s *waStore) PutNCTSalt(ctx context.Context, salt []byte) error { return s.kv.put(ctx, "nct", salt, 0) }

func (s *waStore) GetNCTSalt(ctx context.Context) ([]byte, error) {
	value, _, err := s.kv.get(ctx, "nct")
	return value, err
}

func (s *waStore) DeleteNCTSalt(ctx context.Context) error { return s.kv.delete(ctx, "nct") }

// Event buffer. WhatsApp's servers keep events for 14 days and retries for 7;
// the database removes expired keys, so the Delete* calls have nothing to do.

const (
	bufferedEventLife = 14 * 24 * time.Hour
	outgoingEventLife = 7 * 24 * time.Hour
)

type storedBufferedEvent struct {
	Plaintext []byte `json:"plaintext,omitempty"`
	Server    int64  `json:"server"`
	Inserted  int64  `json:"inserted"`
}

func (s *waStore) GetBufferedEvent(ctx context.Context, hash [32]byte) (*store.BufferedEvent, error) {
	value, ok, err := getJSON[storedBufferedEvent](ctx, s.kv, "evbuf/"+hex.EncodeToString(hash[:]))
	if err != nil || !ok {
		return nil, err
	}
	return &store.BufferedEvent{Plaintext: value.Plaintext, ServerTime: time.Unix(value.Server, 0), InsertTime: time.UnixMilli(value.Inserted)}, nil
}

func (s *waStore) PutBufferedEvent(ctx context.Context, hash [32]byte, plaintext []byte, serverTimestamp time.Time) error {
	value := storedBufferedEvent{Plaintext: plaintext, Server: serverTimestamp.Unix(), Inserted: time.Now().UnixMilli()}
	return putJSON(ctx, s.kv, "evbuf/"+hex.EncodeToString(hash[:]), value, bufferedEventLife)
}

func (s *waStore) DoDecryptionTxn(ctx context.Context, fn func(context.Context) error) error {
	return s.kv.transaction(ctx, fn)
}

func (s *waStore) ClearBufferedEventPlaintext(ctx context.Context, hash [32]byte) error {
	name := "evbuf/" + hex.EncodeToString(hash[:])
	value, ok, err := getJSON[storedBufferedEvent](ctx, s.kv, name)
	if err != nil || !ok || value.Plaintext == nil {
		return err
	}
	value.Plaintext = nil
	left := time.Until(time.UnixMilli(value.Inserted).Add(bufferedEventLife))
	if left <= 0 {
		return s.kv.delete(ctx, name)
	}
	return putJSON(ctx, s.kv, name, value, left)
}

func (s *waStore) DeleteOldBufferedHashes(ctx context.Context) error { return nil }

type storedOutgoingEvent struct {
	Format    string `json:"format"`
	Plaintext []byte `json:"plaintext"`
}

func (s *waStore) GetOutgoingEvent(ctx context.Context, chatJID, altChatJID types.JID, id types.MessageID) (string, []byte, error) {
	for _, chat := range []types.JID{chatJID, altChatJID} {
		if chat.IsEmpty() {
			continue
		}
		value, ok, err := getJSON[storedOutgoingEvent](ctx, s.kv, "out/"+chat.String()+"/"+id)
		if err != nil {
			return "", nil, err
		} else if ok {
			return value.Format, value.Plaintext, nil
		}
	}
	return "", nil, nil
}

func (s *waStore) AddOutgoingEvent(ctx context.Context, chatJID types.JID, id types.MessageID, format string, plaintext []byte) error {
	return putJSON(ctx, s.kv, "out/"+chatJID.String()+"/"+id, storedOutgoingEvent{format, plaintext}, outgoingEventLife)
}

func (s *waStore) DeleteOldOutgoingEvents(ctx context.Context) error { return nil }

// LID map: phone numbers and LIDs of the same user, both directions in memory.

func (s *waStore) lookupLID(ctx context.Context, from types.JID, direction, targetServer string) types.JID {
	value, ok, _ := s.kv.get(ctx, "lid/"+direction+"/"+from.User)
	if !ok || len(value) == 0 {
		return types.JID{}
	}
	return types.JID{User: string(value), Device: from.Device, Server: targetServer}
}

func (s *waStore) GetLIDForPN(ctx context.Context, pn types.JID) (types.JID, error) {
	if pn.Server != types.DefaultUserServer {
		return types.JID{}, fmt.Errorf("invalid GetLIDForPN call with non-PN JID %s", pn)
	}
	return s.lookupLID(ctx, pn, "pn", types.HiddenUserServer), nil
}

func (s *waStore) GetPNForLID(ctx context.Context, lid types.JID) (types.JID, error) {
	if lid.Server != types.HiddenUserServer {
		return types.JID{}, fmt.Errorf("invalid GetPNForLID call with non-LID JID %s", lid)
	}
	return s.lookupLID(ctx, lid, "lid", types.DefaultUserServer), nil
}

func (s *waStore) GetManyLIDsForPNs(ctx context.Context, pns []types.JID) (map[types.JID]types.JID, error) {
	if len(pns) == 0 {
		return nil, nil
	}
	result := make(map[types.JID]types.JID, len(pns))
	for _, pn := range pns {
		if pn.Server != types.DefaultUserServer {
			continue
		}
		if lid := s.lookupLID(ctx, pn, "pn", types.HiddenUserServer); !lid.IsEmpty() {
			result[pn] = lid
		}
	}
	return result, nil
}

func (s *waStore) putLIDMapping(ctx context.Context, lid, pn types.JID) error {
	if lid.Server != types.HiddenUserServer || pn.Server != types.DefaultUserServer {
		return fmt.Errorf("invalid PutLIDMapping call %s/%s", lid, pn)
	}
	oldLID, _, _ := s.kv.get(ctx, "lid/pn/"+pn.User)
	if string(oldLID) == lid.User {
		return nil
	}
	oldPN, _, _ := s.kv.get(ctx, "lid/lid/"+lid.User)
	// One LID per phone number and one phone number per LID.
	if len(oldLID) > 0 {
		if back, _, _ := s.kv.get(ctx, "lid/lid/"+string(oldLID)); string(back) == pn.User {
			_ = s.kv.delete(ctx, "lid/lid/"+string(oldLID))
		}
	}
	if len(oldPN) > 0 && string(oldPN) != pn.User {
		if back, _, _ := s.kv.get(ctx, "lid/pn/"+string(oldPN)); string(back) == lid.User {
			_ = s.kv.delete(ctx, "lid/pn/"+string(oldPN))
		}
	}
	if err := s.kv.put(ctx, "lid/pn/"+pn.User, []byte(lid.User), 0); err != nil {
		return err
	}
	return s.kv.put(ctx, "lid/lid/"+lid.User, []byte(pn.User), 0)
}

func (s *waStore) PutLIDMapping(ctx context.Context, lid, pn types.JID) error {
	s.lidMu.Lock()
	defer s.lidMu.Unlock()
	return s.kv.transaction(ctx, func(ctx context.Context) error { return s.putLIDMapping(ctx, lid, pn) })
}

func (s *waStore) PutManyLIDMappings(ctx context.Context, mappings []store.LIDMapping) error {
	s.lidMu.Lock()
	defer s.lidMu.Unlock()
	return s.kv.transaction(ctx, func(ctx context.Context) error {
		for _, mapping := range mappings {
			if mapping.LID.Server != types.HiddenUserServer || mapping.PN.Server != types.DefaultUserServer {
				continue
			}
			if err := s.putLIDMapping(ctx, mapping.LID, mapping.PN); err != nil {
				return err
			}
		}
		return nil
	})
}
