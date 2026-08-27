package carddav

import (
	"strings"
)

// Resource kind for a DAV path under /dav.
type resKind int

const (
	resNone resKind = iota
	resRoot
	resPrincipal
	resHome
	resBook
	resCard
)

// davPath is a parsed /dav/... path.
type davPath struct {
	Kind   resKind
	UID    string
	BookID string
	CardID string // without .vcf
}

// parseDAVPath parses URL path after optional host. Accepts with or without /dav prefix.
func parseDAVPath(raw string) davPath {
	p := strings.Trim(raw, "/")
	// Strip leading "dav" if present.
	if p == "dav" || p == "" {
		return davPath{Kind: resRoot}
	}
	if strings.HasPrefix(p, "dav/") {
		p = strings.TrimPrefix(p, "dav/")
		p = strings.Trim(p, "/")
	}
	if p == "" {
		return davPath{Kind: resRoot}
	}

	parts := strings.Split(p, "/")
	switch parts[0] {
	case "principals":
		if len(parts) < 2 || parts[1] == "" {
			return davPath{Kind: resNone}
		}
		return davPath{Kind: resPrincipal, UID: parts[1]}
	case "addressbooks":
		if len(parts) < 2 || parts[1] == "" {
			return davPath{Kind: resNone}
		}
		uid := parts[1]
		if len(parts) == 2 {
			return davPath{Kind: resHome, UID: uid}
		}
		book := parts[2]
		if book == "" {
			return davPath{Kind: resHome, UID: uid}
		}
		if len(parts) == 3 {
			return davPath{Kind: resBook, UID: uid, BookID: book}
		}
		// card: .../book/id.vcf or .../book/id
		card := parts[3]
		card = strings.TrimSuffix(card, ".vcf")
		if card == "" {
			return davPath{Kind: resBook, UID: uid, BookID: book}
		}
		return davPath{Kind: resCard, UID: uid, BookID: book, CardID: card}
	default:
		return davPath{Kind: resNone}
	}
}

func principalHref(uid string) string {
	return "/dav/principals/" + uid + "/"
}

func homeHref(uid string) string {
	return "/dav/addressbooks/" + uid + "/"
}

func bookHref(uid, book string) string {
	return "/dav/addressbooks/" + uid + "/" + book + "/"
}

func cardHref(uid, book, id string) string {
	return "/dav/addressbooks/" + uid + "/" + book + "/" + id + ".vcf"
}
