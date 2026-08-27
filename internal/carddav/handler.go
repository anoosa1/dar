package carddav

import (
	"io"
	"net/http"
	"strings"
)

// Handler serves CardDAV under /dav/ (and optional well-known redirect target).
type Handler struct {
	Backend Backend
	// UserID returns the authenticated user id from the outer server context.
	UserID func(*http.Request) string
	// MaxCardBytes rejects PUT larger than this (default 1 MiB).
	MaxCardBytes int64
}

func (h *Handler) authUID(r *http.Request) string {
	if h.UserID != nil {
		return h.UserID(r)
	}
	return ""
}

func (h *Handler) maxBytes() int64 {
	if h.MaxCardBytes > 0 {
		return h.MaxCardBytes
	}
	return 1 << 20 // 1 MiB
}

// ServeHTTP implements http.Handler.
func (h *Handler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if h.Backend == nil {
		http.Error(w, "carddav not configured", http.StatusInternalServerError)
		return
	}

	// Well-known may be proxied here as path /.well-known/carddav
	if strings.Contains(r.URL.Path, ".well-known/carddav") {
		http.Redirect(w, r, "/dav/", http.StatusMovedPermanently)
		return
	}

	auth := h.authUID(r)
	if auth == "" {
		w.Header().Set("WWW-Authenticate", `Basic realm="Dar"`)
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}

	path := parseDAVPath(r.URL.Path)

	// Enforce path uid == auth (except root which has no uid).
	if path.UID != "" && path.UID != auth {
		http.NotFound(w, r)
		return
	}
	// Force uid to auth for root-derived links.
	if path.UID == "" && path.Kind != resRoot && path.Kind != resNone {
		http.NotFound(w, r)
		return
	}

	switch r.Method {
	case http.MethodOptions:
		h.handleOptions(w)
	case "PROPFIND":
		h.handlePropfind(w, r, path, auth)
	case "REPORT":
		h.handleReport(w, r, path, auth)
	case http.MethodGet, http.MethodHead:
		h.handleGet(w, r, path, auth)
	case http.MethodPut:
		h.handlePut(w, r, path, auth)
	case http.MethodDelete:
		h.handleDelete(w, r, path, auth)
	default:
		w.Header().Set("Allow", "OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, REPORT")
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func (h *Handler) handleOptions(w http.ResponseWriter) {
	w.Header().Set("Allow", "OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, REPORT")
	w.Header().Set("DAV", "1, 3, addressbook")
	w.Header().Set("MS-Author-Via", "DAV")
	w.WriteHeader(http.StatusOK)
}

func (h *Handler) handlePropfind(w http.ResponseWriter, r *http.Request, path davPath, auth string) {
	body, _ := io.ReadAll(io.LimitReader(r.Body, 1<<20))
	req := parsePropfind(body)
	depth := r.Header.Get("Depth")
	if depth == "" {
		depth = "0"
	}

	var parts []string
	parts = append(parts, msOpen())

	switch path.Kind {
	case resRoot:
		parts = append(parts, responseOK("/dav/", h.propRoot(req, auth)))
		if depth != "0" {
			// Include principal as child for convenience.
			parts = append(parts, responseOK(principalHref(auth), h.propPrincipal(req, auth)))
		}
	case resPrincipal:
		parts = append(parts, responseOK(principalHref(auth), h.propPrincipal(req, auth)))
	case resHome:
		parts = append(parts, responseOK(homeHref(auth), h.propHome(req, auth)))
		if depth != "0" {
			books, err := h.Backend.ListBooks(auth)
			if err != nil {
				http.Error(w, "list books", http.StatusInternalServerError)
				return
			}
			for _, b := range books {
				parts = append(parts, responseOK(bookHref(auth, b.ID), h.propBook(req, b)))
			}
		}
	case resBook:
		book, err := h.Backend.BookMeta(auth, path.BookID)
		if err != nil {
			if err == ErrNotFound {
				http.NotFound(w, r)
				return
			}
			http.Error(w, "book meta", http.StatusInternalServerError)
			return
		}
		parts = append(parts, responseOK(bookHref(auth, book.ID), h.propBook(req, book)))
		if depth != "0" {
			cards, err := h.Backend.ListCards(auth, path.BookID)
			if err != nil {
				http.Error(w, "list cards", http.StatusInternalServerError)
				return
			}
			for _, c := range cards {
				parts = append(parts, responseOK(cardHref(auth, path.BookID, c.ID), h.propCard(req, c, false, "")))
			}
		}
	case resCard:
		vcf, etag, err := h.Backend.GetCard(auth, path.BookID, path.CardID)
		if err != nil {
			if err == ErrNotFound {
				http.NotFound(w, r)
				return
			}
			http.Error(w, "get card", http.StatusInternalServerError)
			return
		}
		_ = vcf
		ci := CardInfo{ID: path.CardID, ETag: etag}
		parts = append(parts, responseOK(cardHref(auth, path.BookID, path.CardID), h.propCard(req, ci, false, "")))
	default:
		http.NotFound(w, r)
		return
	}

	parts = append(parts, msClose())
	writeMultistatus(w, strings.Join(parts, ""))
}

func (h *Handler) propRoot(req propfindRequest, auth string) string {
	var b strings.Builder
	if req.wants("resourcetype") {
		b.WriteString(`<d:resourcetype><d:collection/></d:resourcetype>`)
	}
	if req.wants("current-user-principal") {
		b.WriteString(`<d:current-user-principal><d:href>`)
		b.WriteString(xmlEscape(principalHref(auth)))
		b.WriteString(`</d:href></d:current-user-principal>`)
	}
	if req.wants("displayname") {
		b.WriteString(`<d:displayname>Dar CardDAV</d:displayname>`)
	}
	return b.String()
}

func (h *Handler) propPrincipal(req propfindRequest, auth string) string {
	var b strings.Builder
	if req.wants("resourcetype") {
		b.WriteString(`<d:resourcetype><d:principal/></d:resourcetype>`)
	}
	if req.wants("displayname") {
		name := h.Backend.PrincipalDisplayName(auth)
		if name == "" {
			name = auth
		}
		b.WriteString(`<d:displayname>`)
		b.WriteString(xmlEscape(name))
		b.WriteString(`</d:displayname>`)
	}
	if req.wants("current-user-principal") {
		b.WriteString(`<d:current-user-principal><d:href>`)
		b.WriteString(xmlEscape(principalHref(auth)))
		b.WriteString(`</d:href></d:current-user-principal>`)
	}
	if req.wants("principal-url") {
		b.WriteString(`<d:principal-URL><d:href>`)
		b.WriteString(xmlEscape(principalHref(auth)))
		b.WriteString(`</d:href></d:principal-URL>`)
	}
	if req.wants("addressbook-home-set") {
		b.WriteString(`<card:addressbook-home-set><d:href>`)
		b.WriteString(xmlEscape(homeHref(auth)))
		b.WriteString(`</d:href></card:addressbook-home-set>`)
	}
	return b.String()
}

func (h *Handler) propHome(req propfindRequest, auth string) string {
	var b strings.Builder
	if req.wants("resourcetype") {
		b.WriteString(`<d:resourcetype><d:collection/></d:resourcetype>`)
	}
	if req.wants("displayname") {
		b.WriteString(`<d:displayname>Address books</d:displayname>`)
	}
	if req.wants("current-user-principal") {
		b.WriteString(`<d:current-user-principal><d:href>`)
		b.WriteString(xmlEscape(principalHref(auth)))
		b.WriteString(`</d:href></d:current-user-principal>`)
	}
	return b.String()
}

func (h *Handler) propBook(req propfindRequest, book BookInfo) string {
	var b strings.Builder
	if req.wants("resourcetype") {
		b.WriteString(`<d:resourcetype><d:collection/><card:addressbook/></d:resourcetype>`)
	}
	if req.wants("displayname") {
		b.WriteString(`<d:displayname>`)
		b.WriteString(xmlEscape(book.DisplayName))
		b.WriteString(`</d:displayname>`)
	}
	if req.wants("getctag") {
		ctag := book.CTag
		if ctag == "" {
			ctag = "0"
		}
		b.WriteString(`<cs:getctag>`)
		b.WriteString(xmlEscape(ctag))
		b.WriteString(`</cs:getctag>`)
	}
	if req.wants("supported-address-data") {
		b.WriteString(`<card:supported-address-data>`)
		b.WriteString(`<card:address-data-type content-type="text/vcard" version="3.0"/>`)
		b.WriteString(`</card:supported-address-data>`)
	}
	return b.String()
}

func (h *Handler) propCard(req propfindRequest, c CardInfo, withData bool, data string) string {
	var b strings.Builder
	if req.wants("getetag") || withData {
		b.WriteString(`<d:getetag>`)
		b.WriteString(xmlEscape(c.ETag))
		b.WriteString(`</d:getetag>`)
	}
	if req.wants("getcontenttype") {
		b.WriteString(`<d:getcontenttype>text/vcard; charset=utf-8</d:getcontenttype>`)
	}
	if req.wants("resourcetype") {
		b.WriteString(`<d:resourcetype/>`)
	}
	if withData || req.wants("address-data") {
		// CDATA keeps vCard readable; escape only if payload contains "]]>".
		b.WriteString(`<card:address-data>`)
		if strings.Contains(data, "]]>") {
			b.WriteString(xmlEscape(data))
		} else {
			b.WriteString(`<![CDATA[`)
			b.WriteString(data)
			b.WriteString(`]]>`)
		}
		b.WriteString(`</card:address-data>`)
	}
	return b.String()
}

func (h *Handler) handleGet(w http.ResponseWriter, r *http.Request, path davPath, auth string) {
	if path.Kind != resCard {
		http.Error(w, "not a card", http.StatusMethodNotAllowed)
		return
	}
	vcf, etag, err := h.Backend.GetCard(auth, path.BookID, path.CardID)
	if err != nil {
		if err == ErrNotFound {
			http.NotFound(w, r)
			return
		}
		http.Error(w, "get card", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "text/vcard; charset=utf-8")
	w.Header().Set("ETag", etag)
	if r.Method == http.MethodHead {
		w.WriteHeader(http.StatusOK)
		return
	}
	w.Write(vcf)
}

func (h *Handler) handlePut(w http.ResponseWriter, r *http.Request, path davPath, auth string) {
	if path.Kind != resCard {
		http.Error(w, "put only on cards", http.StatusMethodNotAllowed)
		return
	}
	// Ensure book exists.
	if _, err := h.Backend.BookMeta(auth, path.BookID); err != nil {
		if err == ErrNotFound {
			http.NotFound(w, r)
			return
		}
		http.Error(w, "book", http.StatusInternalServerError)
		return
	}
	body, err := io.ReadAll(io.LimitReader(r.Body, h.maxBytes()+1))
	if err != nil {
		http.Error(w, "read body", http.StatusBadRequest)
		return
	}
	if int64(len(body)) > h.maxBytes() {
		http.Error(w, "too large", http.StatusRequestEntityTooLarge)
		return
	}
	ifMatch := r.Header.Get("If-Match")
	ifNone := r.Header.Get("If-None-Match")
	etag, created, err := h.Backend.PutCard(auth, path.BookID, path.CardID, body, ifMatch, ifNone)
	if err != nil {
		switch err {
		case ErrNotFound:
			http.NotFound(w, r)
		case ErrPrecondition:
			http.Error(w, "precondition failed", http.StatusPreconditionFailed)
		case ErrInvalidCard:
			http.Error(w, "invalid vcard", http.StatusBadRequest)
		case ErrTooLarge:
			http.Error(w, "too large", http.StatusRequestEntityTooLarge)
		default:
			http.Error(w, "put failed", http.StatusInternalServerError)
		}
		return
	}
	w.Header().Set("ETag", etag)
	if created {
		w.WriteHeader(http.StatusCreated)
	} else {
		w.WriteHeader(http.StatusNoContent)
	}
}

func (h *Handler) handleDelete(w http.ResponseWriter, r *http.Request, path davPath, auth string) {
	if path.Kind != resCard {
		http.Error(w, "delete only on cards", http.StatusMethodNotAllowed)
		return
	}
	err := h.Backend.DeleteCard(auth, path.BookID, path.CardID)
	if err != nil {
		if err == ErrNotFound {
			http.NotFound(w, r)
			return
		}
		http.Error(w, "delete failed", http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) handleReport(w http.ResponseWriter, r *http.Request, path davPath, auth string) {
	if path.Kind != resBook && path.Kind != resHome {
		http.Error(w, "report on book", http.StatusMethodNotAllowed)
		return
	}
	body, _ := io.ReadAll(io.LimitReader(r.Body, 1<<20))
	rep := parseReport(body)

	bookID := path.BookID
	// For multiget, hrefs encode book; for query on book use path.BookID.
	var cards []struct {
		book, id, etag string
		data           []byte
	}

	switch rep.Kind {
	case reportMultiget:
		for _, href := range rep.Hrefs {
			dp := parseDAVPath(href)
			if dp.Kind != resCard || dp.UID != auth {
				continue
			}
			vcf, etag, err := h.Backend.GetCard(auth, dp.BookID, dp.CardID)
			if err != nil {
				continue
			}
			cards = append(cards, struct {
				book, id, etag string
				data           []byte
			}{dp.BookID, dp.CardID, etag, vcf})
		}
	case reportQuery, reportUnknown:
		// v1: all cards in book (or all books if home)
		if path.Kind == resBook {
			list, err := h.Backend.ListCards(auth, bookID)
			if err != nil {
				http.Error(w, "list", http.StatusInternalServerError)
				return
			}
			for _, c := range list {
				vcf, etag, err := h.Backend.GetCard(auth, bookID, c.ID)
				if err != nil {
					continue
				}
				cards = append(cards, struct {
					book, id, etag string
					data           []byte
				}{bookID, c.ID, etag, vcf})
			}
		} else {
			books, err := h.Backend.ListBooks(auth)
			if err != nil {
				http.Error(w, "list books", http.StatusInternalServerError)
				return
			}
			for _, b := range books {
				list, err := h.Backend.ListCards(auth, b.ID)
				if err != nil {
					continue
				}
				for _, c := range list {
					vcf, etag, err := h.Backend.GetCard(auth, b.ID, c.ID)
					if err != nil {
						continue
					}
					cards = append(cards, struct {
						book, id, etag string
						data           []byte
					}{b.ID, c.ID, etag, vcf})
				}
			}
		}
	}

	var parts []string
	parts = append(parts, msOpen())
	// allprop-like for report: always include etag + address-data
	req := propfindRequest{AllProp: true}
	for _, c := range cards {
		ci := CardInfo{ID: c.id, ETag: c.etag}
		parts = append(parts, responseOK(cardHref(auth, c.book, c.id), h.propCard(req, ci, true, string(c.data))))
	}
	parts = append(parts, msClose())
	writeMultistatus(w, strings.Join(parts, ""))
}
