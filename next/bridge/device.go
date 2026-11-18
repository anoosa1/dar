package main

import (
	"context"
	"crypto/rand"
	"encoding/binary"
	"encoding/json"
	"fmt"

	"github.com/google/uuid"
	"go.mau.fi/whatsmeow/proto/waAdv"
	"go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/util/keys"
	waLog "go.mau.fi/whatsmeow/util/log"
)

// storedDevice is the linked device's identity; written once pairing succeeds.
type storedDevice struct {
	ID                    string `json:"id"`
	LID                   string `json:"lid,omitempty"`
	RegistrationID        uint32 `json:"registrationId"`
	NoisePriv             []byte `json:"noisePriv"`
	IdentityPriv          []byte `json:"identityPriv"`
	PreKeyPriv            []byte `json:"preKeyPriv"`
	PreKeyID              uint32 `json:"preKeyId"`
	PreKeySignature       []byte `json:"preKeySignature"`
	AdvSecret             []byte `json:"advSecret"`
	AccountDetails        []byte `json:"accountDetails"`
	AccountSignature      []byte `json:"accountSignature"`
	AccountSignatureKey   []byte `json:"accountSignatureKey"`
	DeviceSignature       []byte `json:"deviceSignature"`
	Platform              string `json:"platform,omitempty"`
	BusinessName          string `json:"businessName,omitempty"`
	PushName              string `json:"pushName,omitempty"`
	FacebookUUID          string `json:"facebookUuid,omitempty"`
	LIDMigrationTimestamp int64  `json:"lidMigrationTimestamp,omitempty"`
	CompanionMetaNonce    string `json:"companionMetaNonce,omitempty"`
}

// container stores one account's device and hands whatsmeow the account's stores.
type container struct {
	kv    *kv
	store *waStore
	log   waLog.Logger
}

var _ store.DeviceContainer = (*container)(nil)

func newContainer(s *kv, log waLog.Logger) *container {
	return &container{kv: s, store: newWAStore(s, log.Sub("Store")), log: log}
}

func randomUint32() uint32 {
	var b [4]byte
	_, _ = rand.Read(b[:])
	return binary.BigEndian.Uint32(b[:])
}

// newDevice creates keys for a device that is not linked yet; nothing is stored until pairing succeeds.
func (c *container) newDevice() *store.Device {
	secret := make([]byte, 32)
	_, _ = rand.Read(secret)
	device := &store.Device{
		Log:            c.log,
		Container:      c,
		NoiseKey:       keys.NewKeyPair(),
		IdentityKey:    keys.NewKeyPair(),
		RegistrationID: randomUint32(),
		AdvSecretKey:   secret,
	}
	device.SignedPreKey = device.IdentityKey.CreateSignedPreKey(1)
	return device
}

// loadDevice returns the stored device, or nil when the account was never linked.
func (c *container) loadDevice(ctx context.Context) (*store.Device, error) {
	value, ok, err := getJSON[storedDevice](ctx, c.kv, "device")
	if err != nil || !ok {
		return nil, err
	}
	if len(value.NoisePriv) != 32 || len(value.IdentityPriv) != 32 || len(value.PreKeyPriv) != 32 || len(value.PreKeySignature) != 64 {
		return nil, errInvalidLength
	}
	id, err := types.ParseJID(value.ID)
	if err != nil {
		return nil, fmt.Errorf("stored device ID: %w", err)
	}
	device := &store.Device{
		Log:            c.log,
		NoiseKey:       keys.NewKeyPairFromPrivateKey([32]byte(value.NoisePriv)),
		IdentityKey:    keys.NewKeyPairFromPrivateKey([32]byte(value.IdentityPriv)),
		SignedPreKey:   &keys.PreKey{KeyPair: *keys.NewKeyPairFromPrivateKey([32]byte(value.PreKeyPriv)), KeyID: value.PreKeyID, Signature: (*[64]byte)(value.PreKeySignature)},
		RegistrationID: value.RegistrationID,
		AdvSecretKey:   value.AdvSecret,
		ID:             &id,
		Account: &waAdv.ADVSignedDeviceIdentity{
			Details: value.AccountDetails, AccountSignature: value.AccountSignature,
			AccountSignatureKey: value.AccountSignatureKey, DeviceSignature: value.DeviceSignature,
		},
		Platform:              value.Platform,
		BusinessName:          value.BusinessName,
		PushName:              value.PushName,
		LIDMigrationTimestamp: value.LIDMigrationTimestamp,
		CompanionMetaNonce:    value.CompanionMetaNonce,
	}
	if value.LID != "" {
		if device.LID, err = types.ParseJID(value.LID); err != nil {
			return nil, fmt.Errorf("stored device LID: %w", err)
		}
	}
	if value.FacebookUUID != "" {
		device.FacebookUUID, _ = uuid.Parse(value.FacebookUUID)
	}
	c.initialize(device)
	return device, nil
}

func (c *container) initialize(device *store.Device) {
	device.SetAllStores(c.store)
	device.LIDs = c.store
	device.Container = c
	device.Initialized = true
}

func (c *container) PutDevice(ctx context.Context, device *store.Device) error {
	if device.ID == nil {
		return fmt.Errorf("device JID must be known before it is stored")
	}
	value := storedDevice{
		ID: device.ID.String(), RegistrationID: device.RegistrationID,
		NoisePriv: device.NoiseKey.Priv[:], IdentityPriv: device.IdentityKey.Priv[:],
		PreKeyPriv: device.SignedPreKey.Priv[:], PreKeyID: device.SignedPreKey.KeyID, PreKeySignature: device.SignedPreKey.Signature[:],
		AdvSecret: device.AdvSecretKey, Platform: device.Platform, BusinessName: device.BusinessName, PushName: device.PushName,
		LIDMigrationTimestamp: device.LIDMigrationTimestamp, CompanionMetaNonce: device.CompanionMetaNonce,
	}
	if device.Account != nil {
		value.AccountDetails, value.AccountSignature = device.Account.Details, device.Account.AccountSignature
		value.AccountSignatureKey, value.DeviceSignature = device.Account.AccountSignatureKey, device.Account.DeviceSignature
	}
	if !device.LID.IsEmpty() {
		value.LID = device.LID.String()
	}
	if device.FacebookUUID != uuid.Nil {
		value.FacebookUUID = device.FacebookUUID.String()
	}
	raw, err := json.Marshal(value)
	if err != nil {
		return err
	}
	if err = c.kv.put(ctx, "device", raw, 0); err != nil {
		return err
	}
	if !device.Initialized {
		c.initialize(device)
	}
	return nil
}

// DeleteDevice runs after WhatsApp logs the device out; the runtime then deletes every key for the account.
func (c *container) DeleteDevice(ctx context.Context, device *store.Device) error {
	if device.ID == nil {
		return fmt.Errorf("device JID must be known before it is deleted")
	}
	return c.kv.delete(ctx, "device")
}
