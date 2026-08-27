// Contacts app backend: vCard storage, REST API, and CardDAV.
package contacts

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/safefile"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/theme"
	"github.com/emersion/go-vcard"
)

var contactsTheme = theme.MochaPink()

// TypedValue is an email, phone, or URL with optional TYPE params.
type TypedValue struct {
	Value string   `json:"value"`
	Types []string `json:"types,omitempty"`
}

// Address is a structured postal address (vCard ADR).
type Address struct {
	Types      []string `json:"types,omitempty"`
	POBox      string   `json:"po_box,omitempty"`
	Extended   string   `json:"extended,omitempty"`
	Street     string   `json:"street,omitempty"`
	Locality   string   `json:"locality,omitempty"`
	Region     string   `json:"region,omitempty"`
	PostalCode string   `json:"postal_code,omitempty"`
	Country    string   `json:"country,omitempty"`
}

// Contact is the JSON API model for a contact.
type Contact struct {
	ID         string `json:"id"`
	UID        string `json:"uid,omitempty"`
	BookID     string `json:"book_id,omitempty"`
	FirstName  string `json:"first_name"`
	LastName   string `json:"last_name"`
	MiddleName string `json:"middle_name,omitempty"`
	Prefix     string `json:"prefix,omitempty"`
	Suffix     string `json:"suffix,omitempty"`
	// Legacy single-value fields (filled from first multi-value entry).
	Email string `json:"email,omitempty"`
	Phone string `json:"phone,omitempty"`
	// Multi-value fields.
	Emails    []TypedValue `json:"emails,omitempty"`
	Phones    []TypedValue `json:"phones,omitempty"`
	URLs      []TypedValue `json:"urls,omitempty"`
	Addresses []Address    `json:"addresses,omitempty"`
	// Single URL kept for test/API compatibility.
	URL      string   `json:"url,omitempty"`
	Org      string   `json:"org,omitempty"`
	Title    string   `json:"title,omitempty"`
	Role     string   `json:"role,omitempty"`
	Notes    string   `json:"notes,omitempty"`
	Birthday string   `json:"birthday,omitempty"`
	Groups   []string `json:"groups,omitempty"`
	Color    string   `json:"color,omitempty"`
	Favorite bool     `json:"favorite"`
	// IsMe marks the user's own contact (from Settings personal info).
	IsMe      bool  `json:"is_me,omitempty"`
	UpdatedAt int64 `json:"updated_at"`
}

func newContactID() (string, error) {
	var raw [16]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return "", fmt.Errorf("generate contact id: %w", err)
	}
	return hex.EncodeToString(raw[:]), nil
}

// AddressBook is a per-user address book (directory under addressbooks/).
type AddressBook struct {
	ID           string `json:"id"`
	Name         string `json:"name"`
	Visible      bool   `json:"visible"`
	ContactCount int    `json:"contact_count"`
}

// bookMeta is stored as meta.json inside each address book directory.
type bookMeta struct {
	Name    string `json:"name"`
	Visible *bool  `json:"visible,omitempty"`
}

// DisplayName returns the best human-readable name for list/detail headers.
func (c Contact) DisplayName() string {
	name := strings.TrimSpace(c.FirstName + " " + c.LastName)
	if name != "" {
		return name
	}
	if c.Org != "" {
		return c.Org
	}
	if c.Email != "" {
		return c.Email
	}
	if len(c.Emails) > 0 && c.Emails[0].Value != "" {
		return c.Emails[0].Value
	}
	return "Unnamed"
}

// SortKey returns a lowercase key for alphabetical sorting.
func (c Contact) SortKey() string {
	return strings.ToLower(c.DisplayName())
}

func (c Contact) preferenceSortKey(sortBy string) string {
	switch sortBy {
	case "last_name":
		if value := strings.TrimSpace(c.LastName); value != "" {
			return strings.ToLower(value + "\x00" + c.FirstName)
		}
	case "first_name":
		if value := strings.TrimSpace(c.FirstName); value != "" {
			return strings.ToLower(value + "\x00" + c.LastName)
		}
	}
	return c.SortKey()
}

func (app *App) booksRoot(uid string) string {
	return filepath.Join(app.dataRoot, uid, "addressbooks")
}

// contactsDir returns the default address book directory (legacy helpers/tests).
func (app *App) contactsDir(uid string) string {
	return app.bookDir(uid, "default")
}

// sanitizeBookID allows only [a-zA-Z0-9_-]; empty → "default".
func (app *App) sanitizeBookID(id string) string {
	id = strings.TrimSpace(id)
	if id == "" {
		return "default"
	}
	var b strings.Builder
	for _, r := range id {
		switch {
		case r >= 'a' && r <= 'z', r >= 'A' && r <= 'Z', r >= '0' && r <= '9', r == '-', r == '_':
			b.WriteRune(r)
		}
	}
	out := b.String()
	if out == "" {
		return "default"
	}
	// Prevent path traversal via ".." etc. after stripping.
	if strings.Contains(out, "..") {
		return "default"
	}
	return out
}

func (app *App) bookDir(uid, bookID string) string {
	book := app.sanitizeBookID(bookID)
	d := filepath.Join(app.booksRoot(uid), book)
	os.MkdirAll(d, 0700)
	if book == "default" {
		app.migrateLegacyContacts(uid, d)
		// Ensure default has meta if missing.
		if _, err := os.Stat(filepath.Join(d, "meta.json")); err != nil {
			app.writeBookMeta(d, bookMeta{Name: "Default", Visible: app.boolPtr(true)})
		}
	}
	return d
}

func (app *App) boolPtr(v bool) *bool { return &v }

func (app *App) readBookMeta(dir, bookID string) bookMeta {
	data, err := os.ReadFile(filepath.Join(dir, "meta.json"))
	if err == nil {
		var m bookMeta
		if json.Unmarshal(data, &m) == nil {
			if m.Name == "" {
				m.Name = app.defaultBookName(bookID)
			}
			return m
		}
	}
	return bookMeta{Name: app.defaultBookName(bookID)}
}

func (app *App) writeBookMeta(dir string, m bookMeta) error {
	if m.Name == "" {
		m.Name = "Address book"
	}
	data, err := json.MarshalIndent(m, "", "  ")
	if err != nil {
		return err
	}
	return safefile.Write(filepath.Join(dir, "meta.json"), data)
}

func (app *App) defaultBookName(bookID string) string {
	if bookID == "default" {
		return "Default"
	}
	// Title-case slug: "my-work" → "My work"
	parts := strings.Split(bookID, "-")
	for i, p := range parts {
		if p == "" {
			continue
		}
		parts[i] = strings.ToUpper(p[:1]) + p[1:]
	}
	return strings.Join(parts, " ")
}

func (app *App) countVCards(dir string) int {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return 0
	}
	n := 0
	for _, e := range entries {
		if !e.IsDir() && strings.HasSuffix(e.Name(), ".vcf") {
			n++
		}
	}
	return n
}

func (app *App) bookVisible(m bookMeta) bool {
	if m.Visible == nil {
		return true
	}
	return *m.Visible
}

// ensureDefaultBook creates the default address book directory for a user.
func (app *App) ensureDefaultBook(uid string) {
	app.bookDir(uid, "default")
}

// listBookDirs returns sanitized book ids (directory names) under the user.
func (app *App) listBookDirs(uid string) []string {
	app.ensureDefaultBook(uid)
	root := app.booksRoot(uid)
	entries, err := os.ReadDir(root)
	if err != nil {
		return []string{"default"}
	}
	var ids []string
	seen := map[string]bool{}
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		id := app.sanitizeBookID(e.Name())
		if id != e.Name() {
			// Directory name had invalid chars — skip to avoid confusion.
			continue
		}
		if seen[id] {
			continue
		}
		seen[id] = true
		ids = append(ids, id)
	}
	if !seen["default"] {
		ids = append(ids, "default")
	}
	sort.Strings(ids)
	return ids
}

func (app *App) loadAddressBooks(uid string) []AddressBook {
	ids := app.listBookDirs(uid)
	out := make([]AddressBook, 0, len(ids))
	for _, id := range ids {
		dir := app.bookDir(uid, id)
		m := app.readBookMeta(dir, id)
		out = append(out, AddressBook{
			ID:           id,
			Name:         m.Name,
			Visible:      app.bookVisible(m),
			ContactCount: app.countVCards(dir),
		})
	}
	return out
}

func (app *App) slugifyBookName(name string) string {
	name = strings.ToLower(strings.TrimSpace(name))
	var b strings.Builder
	prevDash := false
	for _, r := range name {
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9':
			b.WriteRune(r)
			prevDash = false
		case r == ' ' || r == '-' || r == '_':
			if !prevDash && b.Len() > 0 {
				b.WriteByte('-')
				prevDash = true
			}
		}
	}
	s := strings.Trim(b.String(), "-")
	if s == "" {
		if id, err := newContactID(); err == nil {
			s = "book-" + id
		} else {
			s = "book"
		}
	}
	if s == "default" {
		s = "book-default"
	}
	return s
}

func (app *App) uniqueBookID(uid, base string) string {
	id := app.sanitizeBookID(base)
	if id == "default" {
		id = "book-default"
	}
	root := app.booksRoot(uid)
	candidate := id
	for i := 2; ; i++ {
		if _, err := os.Stat(filepath.Join(root, candidate)); os.IsNotExist(err) {
			return candidate
		}
		candidate = fmt.Sprintf("%s-%d", id, i)
	}
}

func validContactID(id string) bool {
	id = strings.TrimSpace(id)
	if id == "" || id == "." || id == ".." || len(id) > 240 || filepath.Base(id) != id || strings.ContainsAny(id, `/\`) {
		return false
	}
	for _, r := range id {
		if r < 0x20 || r == 0x7f {
			return false
		}
	}
	return true
}

// findContactPath looks up a contact by id across address books (or a specific book).
func (app *App) findContactPath(uid, id, preferredBook string) (path, book string, err error) {
	id = strings.TrimSpace(id)
	if !validContactID(id) {
		return "", "", os.ErrNotExist
	}
	if preferredBook != "" {
		book = app.sanitizeBookID(preferredBook)
		path = filepath.Join(app.bookDir(uid, book), id+".vcf")
		if _, err := os.Stat(path); err == nil {
			return path, book, nil
		}
		return "", "", os.ErrNotExist
	}
	// Prefer default first, then others.
	books := app.listBookDirs(uid)
	// Move default to front.
	sort.SliceStable(books, func(i, j int) bool {
		if books[i] == "default" {
			return true
		}
		if books[j] == "default" {
			return false
		}
		return false
	})
	for _, b := range books {
		p := filepath.Join(app.bookDir(uid, b), id+".vcf")
		if _, err := os.Stat(p); err == nil {
			return p, b, nil
		}
	}
	return "", "", os.ErrNotExist
}

// migrateLegacyContacts moves data/<uid>/contacts/*.vcf into the default address book once.
func (app *App) migrateLegacyContacts(uid, dest string) {
	legacy := filepath.Join(app.dataRoot, uid, "contacts")
	entries, err := os.ReadDir(legacy)
	if err != nil {
		return
	}
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".vcf") {
			continue
		}
		src := filepath.Join(legacy, e.Name())
		dst := filepath.Join(dest, e.Name())
		if _, err := os.Stat(dst); err == nil {
			continue // already present
		}
		data, err := os.ReadFile(src)
		if err != nil {
			continue
		}
		if err := safefile.Write(dst, data); err == nil {
			os.Remove(src)
		}
	}
}

func (app *App) bookParam(r *http.Request) string {
	if b := r.URL.Query().Get("book"); b != "" {
		return app.sanitizeBookID(b)
	}
	return ""
}

func (app *App) userID(r *http.Request) string {
	if user := server.GetUser(r); user != nil {
		return user.ID
	}
	return ""
}

var colors = []string{
	contactsTheme.Pink,
	contactsTheme.Red,
	contactsTheme.Teal,
	contactsTheme.Mauve,
	contactsTheme.Sapphire,
	contactsTheme.Green,
	contactsTheme.Peach,
	contactsTheme.Sky,
}

func (app *App) contactColor(idx int) string { return colors[idx%len(colors)] }

func (app *App) fieldTypes(f *vcard.Field) []string {
	if f == nil || f.Params == nil {
		return nil
	}
	types := f.Params.Types()
	if len(types) > 0 {
		return types
	}
	// vCard 2.1 bare params: ADR;HOME: / TEL;CELL:
	known := map[string]bool{
		"home": true, "work": true, "cell": true, "fax": true,
		"voice": true, "pref": true, "internet": true, "text": true,
		"pager": true, "video": true, "other": true, "mobile": true,
	}
	for k := range f.Params {
		lk := strings.ToLower(k)
		if known[lk] {
			types = append(types, lk)
		}
	}
	return types
}

func (app *App) parseTypedFields(card vcard.Card, field string) []TypedValue {
	var out []TypedValue
	for _, f := range card[field] {
		if f == nil || strings.TrimSpace(f.Value) == "" {
			continue
		}
		out = append(out, TypedValue{Value: f.Value, Types: app.fieldTypes(f)})
	}
	return out
}

func (app *App) contactFromCard(card vcard.Card, id string, updatedAt int64) Contact {
	c := Contact{
		ID:        id,
		UID:       card.Value(vcard.FieldUID),
		Groups:    []string{},
		Emails:    []TypedValue{},
		Phones:    []TypedValue{},
		Addresses: []Address{},
		UpdatedAt: updatedAt,
	}

	if n := card.Name(); n != nil {
		c.LastName = n.FamilyName
		c.FirstName = n.GivenName
		c.MiddleName = n.AdditionalName
		c.Prefix = n.HonorificPrefix
		c.Suffix = n.HonorificSuffix
	}
	// If N is empty, fall back to FN split.
	if c.FirstName == "" && c.LastName == "" {
		fn := card.PreferredValue(vcard.FieldFormattedName)
		parts := strings.Fields(fn)
		if len(parts) == 1 {
			c.FirstName = parts[0]
		} else if len(parts) > 1 {
			c.FirstName = parts[0]
			c.LastName = strings.Join(parts[1:], " ")
		}
	}

	c.Emails = app.parseTypedFields(card, vcard.FieldEmail)
	c.Phones = app.parseTypedFields(card, vcard.FieldTelephone)
	c.URLs = app.parseTypedFields(card, vcard.FieldURL)
	if len(c.Emails) > 0 {
		c.Email = c.Emails[0].Value
	}
	if len(c.Phones) > 0 {
		c.Phone = c.Phones[0].Value
	}
	if len(c.URLs) > 0 {
		c.URL = c.URLs[0].Value
	}

	for _, a := range card.Addresses() {
		if a == nil {
			continue
		}
		addr := Address{
			Types:      app.fieldTypes(a.Field),
			POBox:      a.PostOfficeBox,
			Extended:   a.ExtendedAddress,
			Street:     a.StreetAddress,
			Locality:   a.Locality,
			Region:     a.Region,
			PostalCode: a.PostalCode,
			Country:    a.Country,
		}
		// Skip completely empty addresses.
		if addr.Street == "" && addr.Locality == "" && addr.PostalCode == "" &&
			addr.Country == "" && addr.POBox == "" && addr.Extended == "" {
			continue
		}
		c.Addresses = append(c.Addresses, addr)
	}

	c.Org = card.Value(vcard.FieldOrganization)
	// ORG can be "Org;Department" — keep full string for now (matches NC company field).
	if i := strings.Index(c.Org, ";"); i >= 0 {
		c.Org = c.Org[:i]
	}
	c.Title = card.Value(vcard.FieldTitle)
	c.Role = card.Value(vcard.FieldRole)
	c.Notes = card.Value(vcard.FieldNote)
	c.Birthday = card.Value(vcard.FieldBirthday)

	for _, g := range card.Categories() {
		g = strings.TrimSpace(g)
		if g != "" {
			c.Groups = append(c.Groups, g)
		}
	}

	if f := card.Get("X-DAR-FAVORITE"); f != nil && f.Value == "1" {
		c.Favorite = true
	}
	if f := card.Get("X-DAR-ME"); f != nil && f.Value == "1" {
		c.IsMe = true
	}

	if c.FirstName == "" && c.LastName == "" && c.Org == "" {
		if c.Email != "" {
			c.FirstName = c.Email
		}
	}
	return c
}

// preprocessVCard21 rewrites bare type params (ADR;HOME: / TEL;CELL:) to
// TYPE= form so go-vcard does not swallow ADR component semicolons as params.
func (app *App) preprocessVCard21(data []byte) []byte {
	known := map[string]bool{
		"home": true, "work": true, "cell": true, "fax": true,
		"voice": true, "pref": true, "internet": true, "mobile": true,
		"pager": true, "video": true, "text": true, "other": true,
	}
	lines := strings.Split(string(data), "\n")
	for i, line := range lines {
		raw := strings.TrimRight(line, "\r")
		// PROPERTY;BARETYPE:value  (vCard 2.1)
		semi := strings.IndexByte(raw, ';')
		colon := strings.IndexByte(raw, ':')
		if semi < 0 || colon < 0 || colon < semi {
			continue
		}
		if strings.Contains(strings.ToUpper(raw[:colon]), "TYPE=") {
			continue
		}
		prop := strings.ToUpper(raw[:semi])
		switch prop {
		case "ADR", "TEL", "EMAIL", "URL", "LABEL":
		default:
			continue
		}
		// Single bare param between ; and :
		param := raw[semi+1 : colon]
		if strings.ContainsAny(param, "=;") {
			continue // multi-param or already key=value
		}
		if known[strings.ToLower(param)] {
			lines[i] = raw[:semi+1] + "TYPE=" + param + raw[colon:]
			if strings.HasSuffix(line, "\r") {
				// preserve nothing special; we join with \n
			}
		}
	}
	return []byte(strings.Join(lines, "\n"))
}

func (app *App) decodeCard(data []byte) (vcard.Card, error) {
	data = app.preprocessVCard21(data)
	dec := vcard.NewDecoder(bytes.NewReader(data))
	return dec.Decode()
}

func (app *App) parseVCard(path string) (Contact, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return Contact{}, err
	}
	card, err := app.decodeCard(data)
	if err != nil {
		return Contact{}, err
	}
	id := strings.TrimSuffix(filepath.Base(path), ".vcf")
	var updatedAt int64
	if s, err := os.Stat(path); err == nil {
		updatedAt = s.ModTime().Unix()
	}
	return app.contactFromCard(card, id, updatedAt), nil
}

func (app *App) typedField(field, value string, types []string) *vcard.Field {
	f := &vcard.Field{Value: value, Params: make(vcard.Params)}
	for _, t := range types {
		t = strings.TrimSpace(t)
		if t != "" {
			f.Params.Add(vcard.ParamType, t)
		}
	}
	return f
}

func (app *App) toVCard(c Contact) string {
	// Normalize legacy single fields into multi-value slices.
	emails := c.Emails
	if len(emails) == 0 && c.Email != "" {
		emails = []TypedValue{{Value: c.Email}}
	}
	phones := c.Phones
	if len(phones) == 0 && c.Phone != "" {
		phones = []TypedValue{{Value: c.Phone}}
	}
	urls := c.URLs
	if len(urls) == 0 && c.URL != "" {
		urls = []TypedValue{{Value: c.URL}}
	}

	card := make(vcard.Card)
	card.SetValue(vcard.FieldVersion, "3.0")

	uid := c.UID
	if uid == "" {
		uid = c.ID
	}
	if uid != "" {
		card.SetValue(vcard.FieldUID, uid)
	}

	fn := strings.TrimSpace(c.FirstName + " " + c.LastName)
	if fn == "" {
		fn = c.DisplayName()
	}
	card.SetValue(vcard.FieldFormattedName, fn)
	card.SetName(&vcard.Name{
		FamilyName:      c.LastName,
		GivenName:       c.FirstName,
		AdditionalName:  c.MiddleName,
		HonorificPrefix: c.Prefix,
		HonorificSuffix: c.Suffix,
	})

	for _, e := range emails {
		if e.Value == "" {
			continue
		}
		card.Add(vcard.FieldEmail, app.typedField(vcard.FieldEmail, e.Value, e.Types))
	}
	for _, p := range phones {
		if p.Value == "" {
			continue
		}
		card.Add(vcard.FieldTelephone, app.typedField(vcard.FieldTelephone, p.Value, p.Types))
	}
	for _, u := range urls {
		if u.Value == "" {
			continue
		}
		card.Add(vcard.FieldURL, app.typedField(vcard.FieldURL, u.Value, u.Types))
	}
	for _, a := range c.Addresses {
		addr := &vcard.Address{
			PostOfficeBox:   a.POBox,
			ExtendedAddress: a.Extended,
			StreetAddress:   a.Street,
			Locality:        a.Locality,
			Region:          a.Region,
			PostalCode:      a.PostalCode,
			Country:         a.Country,
			Field:           &vcard.Field{Params: make(vcard.Params)},
		}
		for _, t := range a.Types {
			if t != "" {
				addr.Field.Params.Add(vcard.ParamType, t)
			}
		}
		card.AddAddress(addr)
	}

	if c.Org != "" {
		card.SetValue(vcard.FieldOrganization, c.Org)
	}
	if c.Title != "" {
		card.SetValue(vcard.FieldTitle, c.Title)
	}
	if c.Role != "" {
		card.SetValue(vcard.FieldRole, c.Role)
	}
	if c.Notes != "" {
		card.SetValue(vcard.FieldNote, c.Notes)
	}
	if c.Birthday != "" {
		card.SetValue(vcard.FieldBirthday, c.Birthday)
	}
	if len(c.Groups) > 0 {
		card.SetCategories(c.Groups)
	}
	if c.Favorite {
		card.SetValue("X-DAR-FAVORITE", "1")
	}
	if c.IsMe {
		card.SetValue("X-DAR-ME", "1")
	}
	card.SetRevision(time.Now().UTC())

	var buf bytes.Buffer
	if err := vcard.NewEncoder(&buf).Encode(card); err != nil {
		// Fallback minimal card
		return "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:" + fn + "\r\nEND:VCARD\r\n"
	}
	return buf.String()
}

func (app *App) isBlankContact(c Contact) bool {
	if c.FirstName != "" || c.LastName != "" || c.Org != "" || c.Title != "" {
		return false
	}
	if c.Email != "" || c.Phone != "" || c.URL != "" || c.Notes != "" {
		return false
	}
	if len(c.Emails) > 0 || len(c.Phones) > 0 || len(c.Addresses) > 0 || len(c.URLs) > 0 {
		return false
	}
	return true
}

func (app *App) normalizeIncoming(c *Contact) {
	if len(c.Emails) == 0 && c.Email != "" {
		c.Emails = []TypedValue{{Value: c.Email}}
	}
	if len(c.Phones) == 0 && c.Phone != "" {
		c.Phones = []TypedValue{{Value: c.Phone}}
	}
	if len(c.URLs) == 0 && c.URL != "" {
		c.URLs = []TypedValue{{Value: c.URL}}
	}
	if len(c.Emails) > 0 {
		c.Email = c.Emails[0].Value
	}
	if len(c.Phones) > 0 {
		c.Phone = c.Phones[0].Value
	}
	if len(c.URLs) > 0 {
		c.URL = c.URLs[0].Value
	}
	if c.Groups == nil {
		c.Groups = []string{}
	}
}

func (app *App) handleList(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	limit, _ := strconv.Atoi(r.URL.Query().Get("limit"))
	if limit <= 0 {
		limit = 100
	}
	if limit > 200 {
		limit = 200
	}
	offset, _ := strconv.Atoi(r.URL.Query().Get("offset"))
	if offset < 0 {
		offset = 0
	}
	bookFilter := app.bookParam(r) // empty = all visible books
	type contactRow struct {
		Contact
		Initial string `json:"initial"`
	}
	var contacts []contactRow
	var books []string
	if bookFilter != "" {
		books = []string{bookFilter}
	} else {
		// Only books marked visible (Show toggle in settings).
		for _, b := range app.loadAddressBooks(uid) {
			if b.Visible {
				books = append(books, b.ID)
			}
		}
		// No fallback: hiding every book (including default) yields an empty list.
	}
	i := 0
	for _, book := range books {
		dir := app.bookDir(uid, book)
		entries, _ := os.ReadDir(dir)
		for _, e := range entries {
			if !strings.HasSuffix(e.Name(), ".vcf") {
				continue
			}
			c, err := app.parseVCard(filepath.Join(dir, e.Name()))
			if err != nil {
				continue
			}
			c.BookID = book
			c.Color = app.contactColor(i)
			i++
			dn := c.DisplayName()
			initial := ""
			if dn != "" {
				initial = strings.ToUpper(string([]rune(dn)[0]))
			}
			contacts = append(contacts, contactRow{Contact: c, Initial: initial})
		}
	}
	preference, _ := app.loadPreferences(r)
	sort.SliceStable(contacts, func(i, j int) bool {
		return contacts[i].preferenceSortKey(preference.SortBy) < contacts[j].preferenceSortKey(preference.SortBy)
	})
	if offset >= len(contacts) {
		contacts = nil
	} else {
		contacts = contacts[offset:]
		if len(contacts) > limit {
			contacts = contacts[:limit]
		}
	}
	if contacts == nil {
		contacts = []contactRow{}
	}
	app.writeJSON(w, map[string]any{"contacts": contacts})
}

func (app *App) handleGet(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	id := r.URL.Query().Get("id")
	path, book, err := app.findContactPath(uid, id, app.bookParam(r))
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	c, err := app.parseVCard(path)
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	c.BookID = book
	name := strings.TrimSpace(c.FirstName + " " + c.LastName)
	if name == "" {
		name = c.Org
	}
	app.recordActivity(r.Context(), app.userID(r), "contact_created", "created contact "+name, "contact", c.ID, name, "api")
	app.writeJSON(w, c)
}

func (app *App) handleCreate(w http.ResponseWriter, r *http.Request) {
	defer app.markSearchDirty()
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	var c Contact
	if err := json.NewDecoder(r.Body).Decode(&c); err != nil {
		http.Error(w, "bad request", 400)
		return
	}
	app.normalizeIncoming(&c)
	newID, err := newContactID()
	if err != nil {
		http.Error(w, "could not create contact", http.StatusInternalServerError)
		return
	}
	c.ID = newID
	if c.UID == "" {
		c.UID = c.ID
	}
	book := c.BookID
	if book == "" {
		book = app.bookParam(r)
	}
	if book == "" {
		book = "default"
	}
	book = app.sanitizeBookID(book)
	// Ensure book exists (creates dir).
	dir := app.bookDir(app.userID(r), book)
	path := filepath.Join(dir, c.ID+".vcf")
	if err := safefile.Write(path, []byte(app.toVCard(c))); err != nil {
		http.Error(w, "write error", 500)
		return
	}
	// Re-read so response matches on-disk representation.
	if parsed, err := app.parseVCard(path); err == nil {
		c = parsed
	}
	c.BookID = book
	app.writeJSON(w, c)
}

func (app *App) handleUpdate(w http.ResponseWriter, r *http.Request) {
	defer app.markSearchDirty()
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	// Decode into a map first so we know which fields were present.
	var raw map[string]json.RawMessage
	if err := json.NewDecoder(r.Body).Decode(&raw); err != nil {
		http.Error(w, "bad request", 400)
		return
	}
	var incoming Contact
	b, _ := json.Marshal(raw)
	json.Unmarshal(b, &incoming)
	if incoming.ID == "" {
		http.Error(w, "missing id", 400)
		return
	}

	uid := app.userID(r)
	preferred := incoming.BookID
	if preferred == "" {
		preferred = app.bookParam(r)
	}
	fpath, book, err := app.findContactPath(uid, incoming.ID, preferred)
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	existing, err := app.parseVCard(fpath)
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}

	// Scalar merges: only if key present in JSON.
	if _, ok := raw["first_name"]; ok {
		existing.FirstName = incoming.FirstName
	}
	if _, ok := raw["last_name"]; ok {
		existing.LastName = incoming.LastName
	}
	if _, ok := raw["middle_name"]; ok {
		existing.MiddleName = incoming.MiddleName
	}
	if _, ok := raw["prefix"]; ok {
		existing.Prefix = incoming.Prefix
	}
	if _, ok := raw["suffix"]; ok {
		existing.Suffix = incoming.Suffix
	}
	if _, ok := raw["org"]; ok {
		existing.Org = incoming.Org
	}
	if _, ok := raw["title"]; ok {
		existing.Title = incoming.Title
	}
	if _, ok := raw["role"]; ok {
		existing.Role = incoming.Role
	}
	if _, ok := raw["notes"]; ok {
		existing.Notes = incoming.Notes
	}
	if _, ok := raw["url"]; ok {
		existing.URL = incoming.URL
	}
	if _, ok := raw["birthday"]; ok {
		existing.Birthday = incoming.Birthday
	}
	if _, ok := raw["color"]; ok && incoming.Color != "" {
		existing.Color = incoming.Color
	}
	if _, ok := raw["favorite"]; ok {
		existing.Favorite = incoming.Favorite
	}

	// Arrays: replace when present (including empty to clear).
	if _, ok := raw["emails"]; ok {
		existing.Emails = incoming.Emails
		if existing.Emails == nil {
			existing.Emails = []TypedValue{}
		}
		if len(existing.Emails) > 0 {
			existing.Email = existing.Emails[0].Value
		} else {
			existing.Email = ""
		}
	} else if _, ok := raw["email"]; ok {
		existing.Email = incoming.Email
		if incoming.Email != "" {
			existing.Emails = []TypedValue{{Value: incoming.Email}}
		} else {
			existing.Emails = []TypedValue{}
		}
	}
	if _, ok := raw["phones"]; ok {
		existing.Phones = incoming.Phones
		if existing.Phones == nil {
			existing.Phones = []TypedValue{}
		}
		if len(existing.Phones) > 0 {
			existing.Phone = existing.Phones[0].Value
		} else {
			existing.Phone = ""
		}
	} else if _, ok := raw["phone"]; ok {
		existing.Phone = incoming.Phone
		if incoming.Phone != "" {
			existing.Phones = []TypedValue{{Value: incoming.Phone}}
		} else {
			existing.Phones = []TypedValue{}
		}
	}
	if _, ok := raw["addresses"]; ok {
		existing.Addresses = incoming.Addresses
		if existing.Addresses == nil {
			existing.Addresses = []Address{}
		}
	}
	if _, ok := raw["urls"]; ok {
		existing.URLs = incoming.URLs
		if existing.URLs == nil {
			existing.URLs = []TypedValue{}
		}
	}
	if _, ok := raw["groups"]; ok {
		existing.Groups = incoming.Groups
		if existing.Groups == nil {
			existing.Groups = []string{}
		}
	}

	if err := safefile.Write(fpath, []byte(app.toVCard(existing))); err != nil {
		http.Error(w, "write error", 500)
		return
	}
	if parsed, err := app.parseVCard(fpath); err == nil {
		existing = parsed
	}
	existing.BookID = book
	name := strings.TrimSpace(existing.FirstName + " " + existing.LastName)
	if name == "" {
		name = existing.Org
	}
	app.recordActivity(r.Context(), uid, "contact_updated", "updated contact "+name, "contact", existing.ID, name, "api")
	app.writeJSON(w, existing)
}

func (app *App) handleDelete(w http.ResponseWriter, r *http.Request) {
	defer app.markSearchDirty()
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := app.userID(r)
	id := r.URL.Query().Get("id")
	path, _, err := app.findContactPath(uid, id, app.bookParam(r))
	if err != nil {
		http.Error(w, "not found", 404)
		return
	}
	os.Remove(path)
	app.recordActivity(r.Context(), uid, "contact_deleted", "deleted contact "+id, "contact", id, id, "api")
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) contactMatches(c Contact, q string) bool {
	parts := []string{c.FirstName, c.LastName, c.Org, c.Title, c.Notes, c.Email, c.Phone, c.URL, c.Birthday}
	for _, e := range c.Emails {
		parts = append(parts, e.Value)
	}
	for _, p := range c.Phones {
		parts = append(parts, p.Value)
	}
	for _, a := range c.Addresses {
		parts = append(parts, a.Street, a.Locality, a.Region, a.Country)
	}
	for _, g := range c.Groups {
		parts = append(parts, g)
	}
	return strings.Contains(strings.ToLower(strings.Join(parts, " ")), q)
}

func (app *App) handleSearch(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	q := strings.ToLower(r.URL.Query().Get("q"))
	bookFilter := app.bookParam(r)
	limit, _ := strconv.Atoi(r.URL.Query().Get("limit"))
	if limit <= 0 {
		limit = 100
	}
	if limit > 200 {
		limit = 200
	}
	offset, _ := strconv.Atoi(r.URL.Query().Get("offset"))
	if offset < 0 {
		offset = 0
	}
	matched := 0
	var books []string
	if bookFilter != "" {
		books = []string{bookFilter}
	} else {
		for _, b := range app.loadAddressBooks(uid) {
			if b.Visible {
				books = append(books, b.ID)
			}
		}
	}
	var results []Contact
	for _, book := range books {
		dir := app.bookDir(uid, book)
		entries, _ := os.ReadDir(dir)
		for _, e := range entries {
			if !strings.HasSuffix(e.Name(), ".vcf") {
				continue
			}
			c, err := app.parseVCard(filepath.Join(dir, e.Name()))
			if err != nil {
				continue
			}
			c.BookID = book
			if q == "" || app.contactMatches(c, q) {
				if matched < offset {
					matched++
					continue
				}
				results = append(results, c)
				matched++
				if len(results) >= limit {
					break
				}
			}
		}
	}
	if len(results) > limit {
		results = results[:limit]
	}
	if results == nil {
		results = []Contact{}
	}
	app.writeJSON(w, map[string]any{"contacts": results})
}

func (app *App) handleHealth(w http.ResponseWriter, r *http.Request) {
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) handleExport(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	id := r.URL.Query().Get("id")
	bookFilter := app.bookParam(r)

	if id != "" {
		fpath, _, err := app.findContactPath(uid, id, bookFilter)
		if err != nil {
			http.Error(w, "not found", 404)
			return
		}
		data, err := os.ReadFile(fpath)
		if err != nil {
			http.Error(w, "not found", 404)
			return
		}
		w.Header().Set("Content-Type", "text/vcard")
		w.Header().Set("Content-Disposition", fmt.Sprintf(`attachment; filename="%s.vcf"`, id))
		w.Write(data)
		return
	}

	// Export all contacts in one book, or all visible books.
	var books []string
	if bookFilter != "" {
		books = []string{bookFilter}
	} else {
		for _, b := range app.loadAddressBooks(uid) {
			books = append(books, b.ID)
		}
		if len(books) == 0 {
			books = []string{"default"}
		}
	}
	var buf bytes.Buffer
	for _, book := range books {
		dir := app.bookDir(uid, book)
		entries, _ := os.ReadDir(dir)
		for _, e := range entries {
			if !strings.HasSuffix(e.Name(), ".vcf") {
				continue
			}
			data, err := os.ReadFile(filepath.Join(dir, e.Name()))
			if err != nil {
				continue
			}
			buf.Write(data)
			if !bytes.HasSuffix(data, []byte("\n")) {
				buf.WriteByte('\n')
			}
		}
	}
	filename := "contacts.vcf"
	if bookFilter != "" {
		filename = bookFilter + ".vcf"
	}
	w.Header().Set("Content-Type", "text/vcard")
	w.Header().Set("Content-Disposition", fmt.Sprintf(`attachment; filename="%s"`, filename))
	w.Write(buf.Bytes())
}

func (app *App) handleImport(w http.ResponseWriter, r *http.Request) {
	defer app.markSearchDirty()
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, 10<<20)
	if err := r.ParseMultipartForm(10 << 20); err != nil {
		http.Error(w, "missing file", 400)
		return
	}
	file, _, err := r.FormFile("file")
	if err != nil {
		http.Error(w, "missing file", 400)
		return
	}
	defer file.Close()
	data, err := io.ReadAll(file)
	if err != nil {
		http.Error(w, "read error", 500)
		return
	}

	count := 0
	uid := app.userID(r)
	book := app.bookParam(r)
	if book == "" {
		book = "default"
	}
	dir := app.bookDir(uid, book)
	dec := vcard.NewDecoder(bytes.NewReader(app.preprocessVCard21(data)))
	for {
		card, err := dec.Decode()
		if err == io.EOF {
			break
		}
		if err != nil {
			// Try splitting manually if decoder fails mid-stream.
			break
		}
		c := app.contactFromCard(card, "", time.Now().Unix())
		if app.isBlankContact(c) {
			continue
		}
		id, err := newContactID()
		if err != nil {
			http.Error(w, "could not import contact", http.StatusInternalServerError)
			return
		}
		c.ID = id
		if c.UID == "" {
			c.UID = id
		}
		// Prefer writing via toVCard so types/fields are normalized; fall back to raw re-encode.
		if err := safefile.Write(filepath.Join(dir, id+".vcf"), []byte(app.toVCard(c))); err != nil {
			http.Error(w, "write error", 500)
			return
		}
		count++
	}

	// Fallback: if go-vcard decoded nothing, try BEGIN/END split (handles odd files).
	if count == 0 {
		raw := string(data)
		for {
			start := strings.Index(raw, "BEGIN:VCARD")
			if start == -1 {
				start = strings.Index(raw, "BEGIN:vCard")
			}
			if start == -1 {
				break
			}
			end := strings.Index(strings.ToUpper(raw[start:]), "END:VCARD")
			if end == -1 {
				break
			}
			chunk := raw[start : start+end+len("END:VCARD")]
			c, err := app.parseVCardBytes([]byte(chunk))
			if err == nil && !app.isBlankContact(c) {
				id, err := newContactID()
				if err != nil {
					http.Error(w, "could not import contact", http.StatusInternalServerError)
					return
				}
				c.ID = id
				if err := safefile.Write(filepath.Join(dir, id+".vcf"), []byte(app.toVCard(c))); err != nil {
					http.Error(w, "write error", 500)
					return
				}
				count++
			}
			raw = raw[start+end+len("END:VCARD"):]
		}
	}

	app.recordActivity(r.Context(), uid, "contacts_imported", fmt.Sprintf("imported %d contacts", count), "address_book", book, book, "api")
	app.writeJSON(w, map[string]any{"imported": count, "book_id": book})
}

// --- Address books API ---

func (app *App) handleBooks(w http.ResponseWriter, r *http.Request) {
	uid := app.userID(r)
	switch r.Method {
	case http.MethodGet, http.MethodHead:
		app.writeJSON(w, map[string]any{"books": app.loadAddressBooks(uid)})
	case http.MethodPost:
		var body struct {
			Name string `json:"name"`
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil || strings.TrimSpace(body.Name) == "" {
			http.Error(w, "name required", 400)
			return
		}
		name := strings.TrimSpace(body.Name)
		id := app.uniqueBookID(uid, app.slugifyBookName(name))
		dir := app.bookDir(uid, id)
		if err := app.writeBookMeta(dir, bookMeta{Name: name, Visible: app.boolPtr(true)}); err != nil {
			http.Error(w, "write error", 500)
			return
		}
		app.recordActivity(r.Context(), uid, "address_book_created", "created address book "+name, "address_book", id, name, "api")
		app.writeJSON(w, AddressBook{ID: id, Name: name, Visible: true, ContactCount: 0})
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (app *App) handleBooksRename(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	var body struct {
		ID   string `json:"id"`
		Name string `json:"name"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, "bad request", 400)
		return
	}
	id := app.sanitizeBookID(body.ID)
	name := strings.TrimSpace(body.Name)
	if id == "" || name == "" {
		http.Error(w, "id and name required", 400)
		return
	}
	uid := app.userID(r)
	// Book must already exist (except default which is always creatable).
	root := app.booksRoot(uid)
	dir := filepath.Join(root, id)
	if id != "default" {
		if st, err := os.Stat(dir); err != nil || !st.IsDir() {
			http.Error(w, "not found", 404)
			return
		}
	} else {
		dir = app.bookDir(uid, id)
	}
	m := app.readBookMeta(dir, id)
	m.Name = name
	if err := app.writeBookMeta(dir, m); err != nil {
		http.Error(w, "write error", 500)
		return
	}
	app.recordActivity(r.Context(), uid, "address_book_renamed", "renamed address book to "+name, "address_book", id, name, "api")
	app.writeJSON(w, AddressBook{
		ID: id, Name: name, Visible: app.bookVisible(m), ContactCount: app.countVCards(dir),
	})
}

func (app *App) handleBooksDelete(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	var body struct {
		ID string `json:"id"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, "bad request", 400)
		return
	}
	id := app.sanitizeBookID(body.ID)
	if id == "default" {
		http.Error(w, "cannot delete default address book", 400)
		return
	}
	uid := app.userID(r)
	dir := filepath.Join(app.booksRoot(uid), id)
	if st, err := os.Stat(dir); err != nil || !st.IsDir() {
		http.Error(w, "not found", 404)
		return
	}
	if err := os.RemoveAll(dir); err != nil {
		http.Error(w, "delete error", 500)
		return
	}
	app.recordActivity(r.Context(), uid, "address_book_deleted", "deleted address book "+id, "address_book", id, id, "api")
	app.writeJSON(w, map[string]string{"status": "ok"})
}

func (app *App) handleBooksVisibility(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	var body struct {
		ID      string `json:"id"`
		Visible bool   `json:"visible"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, "bad request", 400)
		return
	}
	id := app.sanitizeBookID(body.ID)
	uid := app.userID(r)
	var dir string
	if id == "default" {
		dir = app.bookDir(uid, id)
	} else {
		dir = filepath.Join(app.booksRoot(uid), id)
		if st, err := os.Stat(dir); err != nil || !st.IsDir() {
			http.Error(w, "not found", 404)
			return
		}
	}
	m := app.readBookMeta(dir, id)
	m.Visible = app.boolPtr(body.Visible)
	if err := app.writeBookMeta(dir, m); err != nil {
		http.Error(w, "write error", 500)
		return
	}
	visibility := "hid"
	if body.Visible {
		visibility = "showed"
	}
	app.recordActivity(r.Context(), uid, "address_book_visibility", visibility+" address book "+m.Name, "address_book", id, m.Name, "api")
	app.writeJSON(w, AddressBook{
		ID: id, Name: m.Name, Visible: body.Visible, ContactCount: app.countVCards(dir),
	})
}

// meDisplayName returns "{name} (Me)", avoiding a double suffix.
func (app *App) meDisplayName(name string) string {
	name = strings.TrimSpace(name)
	if name == "" {
		return "Me"
	}
	if strings.HasSuffix(name, " (Me)") || strings.HasSuffix(name, "(Me)") {
		return name
	}
	return name + " (Me)"
}

// findMeContact looks for the contact marked X-DAR-ME in the default book.
func (app *App) findMeContact(uid string) (Contact, string, bool) {
	dir := app.bookDir(uid, "default")
	entries, err := os.ReadDir(dir)
	if err != nil {
		return Contact{}, "", false
	}
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".vcf") {
			continue
		}
		path := filepath.Join(dir, e.Name())
		c, err := app.parseVCard(path)
		if err != nil {
			continue
		}
		if c.IsMe {
			c.BookID = "default"
			return c, path, true
		}
	}
	return Contact{}, "", false
}

// meBody is the personal-info payload from Settings → Me contact.
type meBody struct {
	DisplayName  string `json:"display_name"`
	Email        string `json:"email"`
	Phone        string `json:"phone"`
	Address      string `json:"address"`
	Website      string `json:"website"`
	Twitter      string `json:"twitter"`
	Fediverse    string `json:"fediverse"`
	Organisation string `json:"organisation"`
	Role         string `json:"role"`
	Headline     string `json:"headline"`
	Biography    string `json:"biography"`
	Location     string `json:"location"`
	Birthday     string `json:"birthday"`
}

// applyMeBody maps Nextcloud personal-info fields onto a Contact (vCard).
func (app *App) applyMeBody(c *Contact, body meBody) {
	c.FirstName = app.meDisplayName(body.DisplayName)
	c.LastName = ""
	c.IsMe = true
	c.BookID = "default"

	if body.Email != "" {
		c.Email = body.Email
		c.Emails = []TypedValue{{Value: body.Email, Types: []string{"pref"}}}
	}
	if body.Phone != "" {
		c.Phone = body.Phone
		c.Phones = []TypedValue{{Value: body.Phone, Types: []string{"pref", "cell"}}}
	} else {
		c.Phone = ""
		c.Phones = nil
	}
	if body.Website != "" {
		c.URL = body.Website
		c.URLs = []TypedValue{{Value: body.Website, Types: []string{"pref"}}}
	} else {
		c.URL = ""
		c.URLs = nil
	}
	// Extra social URLs (twitter / fediverse) as additional URL entries.
	if body.Twitter != "" {
		tw := body.Twitter
		if !strings.HasPrefix(tw, "http://") && !strings.HasPrefix(tw, "https://") {
			handle := strings.TrimPrefix(tw, "@")
			tw = "https://twitter.com/" + handle
		}
		c.URLs = append(c.URLs, TypedValue{Value: tw, Types: []string{"twitter"}})
	}
	if body.Fediverse != "" {
		// Prefer a web URL when user already entered one; otherwise store the handle as URL value.
		fed := body.Fediverse
		c.URLs = append(c.URLs, TypedValue{Value: fed, Types: []string{"fediverse"}})
	}
	if body.Address != "" || body.Location != "" {
		c.Addresses = []Address{{
			Types:    []string{"pref"},
			Street:   body.Address,
			Locality: body.Location,
		}}
	} else {
		c.Addresses = nil
	}
	c.Org = body.Organisation
	// Headline maps to TITLE (short tagline); Role maps to ROLE (job role).
	c.Title = body.Headline
	c.Role = body.Role
	c.Birthday = body.Birthday
	c.Notes = body.Biography
}

// UpsertMe synchronizes the authenticated user's personal profile to Contacts.
func (app *App) UpsertMe(ctx context.Context, userID, displayName, email string, profile auth.UserProfile) (string, error) {
	defer app.markSearchDirty()
	if err := ctx.Err(); err != nil {
		return "", err
	}
	body := meBody{
		DisplayName: displayName, Email: email, Phone: profile.Phone,
		Address: profile.Address, Website: profile.Website, Twitter: profile.Twitter,
		Fediverse: profile.Fediverse, Organisation: profile.Organisation, Role: profile.Role,
		Headline: profile.Headline, Biography: profile.Biography, Location: profile.Location,
		Birthday: profile.Birthday,
	}
	existing, path, found := app.findMeContact(userID)
	c := existing
	if !found {
		id, err := newContactID()
		if err != nil {
			return "", err
		}
		c = Contact{ID: id, IsMe: true, BookID: "default"}
		c.UID = c.ID
		path = filepath.Join(app.bookDir(userID, "default"), c.ID+".vcf")
	}
	app.applyMeBody(&c, body)
	app.normalizeIncoming(&c)
	if err := safefile.Write(path, []byte(app.toVCard(c))); err != nil {
		return "", err
	}
	if parsed, err := app.parseVCard(path); err == nil {
		c = parsed
	}
	c.IsMe = true
	if c.FirstName != "" {
		return c.FirstName, nil
	}
	return displayName + " (Me)", nil
}

// handleMe upserts the user's own contact from Settings personal info.
// POST body: Nextcloud personal-info fields (display_name, email, phone, …).
func (app *App) handleMe(w http.ResponseWriter, r *http.Request) {
	defer app.markSearchDirty()
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	var body meBody
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, "bad request", 400)
		return
	}
	if strings.TrimSpace(body.DisplayName) == "" {
		http.Error(w, "display_name required", 400)
		return
	}
	uid := app.userID(r)
	// Put full "Name (Me)" in FirstName so list SortKey/DisplayName work simply.
	// (LastName empty — matches org-style single-field names.)
	existing, path, found := app.findMeContact(uid)
	var c Contact
	if found {
		c = existing
		app.applyMeBody(&c, body)
		app.normalizeIncoming(&c)
		if err := safefile.Write(path, []byte(app.toVCard(c))); err != nil {
			http.Error(w, "write error", 500)
			return
		}
		if parsed, err := app.parseVCard(path); err == nil {
			c = parsed
		}
		c.BookID = "default"
		c.IsMe = true
		app.writeJSON(w, c)
		return
	}

	id, err := newContactID()
	if err != nil {
		http.Error(w, "could not create contact", http.StatusInternalServerError)
		return
	}
	c = Contact{
		ID:     id,
		IsMe:   true,
		BookID: "default",
	}
	app.applyMeBody(&c, body)
	c.UID = c.ID
	app.normalizeIncoming(&c)
	dir := app.bookDir(uid, "default")
	path = filepath.Join(dir, c.ID+".vcf")
	if err := safefile.Write(path, []byte(app.toVCard(c))); err != nil {
		http.Error(w, "write error", 500)
		return
	}
	if parsed, err := app.parseVCard(path); err == nil {
		c = parsed
	}
	c.BookID = "default"
	c.IsMe = true
	app.writeJSON(w, c)
}

func (app *App) parseVCardBytes(data []byte) (Contact, error) {
	card, err := app.decodeCard(data)
	if err != nil {
		return Contact{}, err
	}
	return app.contactFromCard(card, "", 0), nil
}

func (app *App) writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}
