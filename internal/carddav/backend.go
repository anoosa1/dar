// Package carddav implements a CardDAV server (RFC 6352) over a storage Backend.
package carddav

import "errors"

// Sentinel errors returned by Backend implementations.
var (
	ErrNotFound      = errors.New("not found")
	ErrPrecondition  = errors.New("precondition failed") // If-Match / If-None-Match
	ErrInvalidCard   = errors.New("invalid vcard")
	ErrTooLarge      = errors.New("resource too large")
	ErrForbidden     = errors.New("forbidden")
)

// BookInfo describes an address book collection.
type BookInfo struct {
	ID          string // filesystem / path id
	DisplayName string
	CTag        string // calendar-server getctag
}

// CardInfo is a contact resource listing entry.
type CardInfo struct {
	ID   string // filename stem (without .vcf)
	ETag string // quoted etag, e.g. "123-456"
}

// Backend is the storage adapter contacts (or tests) implement.
type Backend interface {
	// ListBooks returns address books for uid.
	ListBooks(uid string) ([]BookInfo, error)
	// BookMeta returns one book; ErrNotFound if missing.
	BookMeta(uid, bookID string) (BookInfo, error)
	// ListCards returns cards in a book.
	ListCards(uid, bookID string) ([]CardInfo, error)
	// GetCard returns raw vCard bytes and etag.
	GetCard(uid, bookID, id string) (vcf []byte, etag string, err error)
	// PutCard creates or replaces a card. ifMatch is the raw If-Match header
	// (may include quotes / *). ifNoneMatch is If-None-Match.
	// Returns new etag and whether the resource was created.
	PutCard(uid, bookID, id string, vcf []byte, ifMatch, ifNoneMatch string) (etag string, created bool, err error)
	// DeleteCard removes a card.
	DeleteCard(uid, bookID, id string) error
	// DisplayName is optional human name for the principal (email or display name).
	// Empty string is fine.
	PrincipalDisplayName(uid string) string
}
