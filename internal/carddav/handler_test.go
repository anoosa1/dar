package carddav

import (
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
)

// memBackend is an in-memory Backend for tests.
type memBackend struct {
	mu    sync.Mutex
	books map[string]BookInfo          // bookID -> info
	cards map[string]map[string][]byte // bookID -> id -> vcf
	etags map[string]map[string]string // bookID -> id -> etag
	seq   int
	fail  string
}

func testHandler() *Handler {
	return &Handler{
		Backend: newMem(),
		UserID:  func(r *http.Request) string { return r.Header.Get("X-User-Id") },
	}
}

func newMem() *memBackend {
	return &memBackend{
		books: map[string]BookInfo{"default": {ID: "default", DisplayName: "Default", CTag: "1"}},
		cards: map[string]map[string][]byte{"default": {}},
		etags: map[string]map[string]string{"default": {}},
	}
}

func (m *memBackend) PrincipalDisplayName(uid string) string { return "Test " + uid }

func (m *memBackend) ListBooks(uid string) ([]BookInfo, error) {
	if m.fail == "books" {
		return nil, errors.New("fixture")
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	var out []BookInfo
	for _, b := range m.books {
		out = append(out, b)
	}
	return out, nil
}

func (m *memBackend) BookMeta(uid, bookID string) (BookInfo, error) {
	if m.fail == "book" {
		return BookInfo{}, errors.New("fixture")
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	b, ok := m.books[bookID]
	if !ok {
		return BookInfo{}, ErrNotFound
	}
	return b, nil
}

func (m *memBackend) ListCards(uid, bookID string) ([]CardInfo, error) {
	if m.fail == "cards" {
		return nil, errors.New("fixture")
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	cm := m.cards[bookID]
	if cm == nil {
		return nil, nil
	}
	var out []CardInfo
	for id := range cm {
		out = append(out, CardInfo{ID: id, ETag: m.etags[bookID][id]})
	}
	return out, nil
}

func (m *memBackend) GetCard(uid, bookID, id string) ([]byte, string, error) {
	if m.fail == "get" {
		return nil, "", errors.New("fixture")
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	cm := m.cards[bookID]
	if cm == nil {
		return nil, "", ErrNotFound
	}
	data, ok := cm[id]
	if !ok {
		return nil, "", ErrNotFound
	}
	return data, m.etags[bookID][id], nil
}

func (m *memBackend) PutCard(uid, bookID, id string, vcf []byte, ifMatch, ifNoneMatch string) (string, bool, error) {
	if m.fail == "put" {
		return "", false, errors.New("fixture")
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	if !strings.Contains(string(vcf), "BEGIN:VCARD") {
		return "", false, ErrInvalidCard
	}
	if m.cards[bookID] == nil {
		return "", false, ErrNotFound
	}
	_, exists := m.cards[bookID][id]
	if ifNoneMatch == "*" && exists {
		return "", false, ErrPrecondition
	}
	if ifMatch != "" && ifMatch != "*" {
		if !exists {
			return "", false, ErrPrecondition
		}
		if m.etags[bookID][id] != ifMatch && unquote(m.etags[bookID][id]) != unquote(ifMatch) {
			return "", false, ErrPrecondition
		}
	}
	m.seq++
	etag := `"e` + strings.Repeat("x", 0) + itoa(m.seq) + `"`
	// simple etag
	etag = `"` + itoa(m.seq) + `"`
	m.cards[bookID][id] = append([]byte(nil), vcf...)
	if m.etags[bookID] == nil {
		m.etags[bookID] = map[string]string{}
	}
	m.etags[bookID][id] = etag
	return etag, !exists, nil
}

func (m *memBackend) DeleteCard(uid, bookID, id string) error {
	if m.fail == "delete" {
		return errors.New("fixture")
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.cards[bookID] == nil {
		return ErrNotFound
	}
	if _, ok := m.cards[bookID][id]; !ok {
		return ErrNotFound
	}
	delete(m.cards[bookID], id)
	delete(m.etags[bookID], id)
	return nil
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var d []byte
	for n > 0 {
		d = append([]byte{byte('0' + n%10)}, d...)
		n /= 10
	}
	return string(d)
}

func unquote(s string) string {
	s = strings.TrimSpace(s)
	if len(s) >= 2 && s[0] == '"' && s[len(s)-1] == '"' {
		return s[1 : len(s)-1]
	}
	return s
}

func withAuth(h http.Handler, uid string) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		r.Header.Set("X-User-Id", uid)
		h.ServeHTTP(w, r)
	})
}

func TestOptions(t *testing.T) {
	h := testHandler()
	rr := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodOptions, "/dav/", nil)
	req.Header.Set("X-User-Id", "alice")
	h.ServeHTTP(rr, req)
	if rr.Code != 200 {
		t.Fatalf("status %d", rr.Code)
	}
	if !strings.Contains(rr.Header().Get("DAV"), "addressbook") {
		t.Fatalf("DAV header %q", rr.Header().Get("DAV"))
	}
}

func TestUnauthorized(t *testing.T) {
	h := testHandler()
	rr := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodOptions, "/dav/", nil)
	h.ServeHTTP(rr, req)
	if rr.Code != 401 {
		t.Fatalf("status %d", rr.Code)
	}
}

func TestPathIsolation(t *testing.T) {
	h := testHandler()
	rr := httptest.NewRecorder()
	req := httptest.NewRequest("PROPFIND", "/dav/addressbooks/bob/", nil)
	req.Header.Set("X-User-Id", "alice")
	req.Header.Set("Depth", "0")
	h.ServeHTTP(rr, req)
	if rr.Code != 404 {
		t.Fatalf("status %d want 404", rr.Code)
	}
}

func TestPropfindRootAndPrincipal(t *testing.T) {
	h := testHandler()
	rr := httptest.NewRecorder()
	req := httptest.NewRequest("PROPFIND", "/dav/", strings.NewReader(`<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>`))
	req.Header.Set("X-User-Id", "alice")
	req.Header.Set("Depth", "0")
	h.ServeHTTP(rr, req)
	if rr.Code != 207 {
		t.Fatalf("status %d body %s", rr.Code, rr.Body.String())
	}
	body := rr.Body.String()
	if !strings.Contains(body, "/dav/principals/alice/") {
		t.Fatalf("missing principal href: %s", body)
	}

	rr2 := httptest.NewRecorder()
	req2 := httptest.NewRequest("PROPFIND", "/dav/principals/alice/", strings.NewReader(`<?xml version="1.0"?><propfind xmlns="DAV:"><prop><addressbook-home-set xmlns="urn:ietf:params:xml:ns:carddav"/></prop></propfind>`))
	req2.Header.Set("X-User-Id", "alice")
	req2.Header.Set("Depth", "0")
	h.ServeHTTP(rr2, req2)
	if rr2.Code != 207 {
		t.Fatalf("principal status %d", rr2.Code)
	}
	if !strings.Contains(rr2.Body.String(), "/dav/addressbooks/alice/") {
		t.Fatalf("missing home-set: %s", rr2.Body.String())
	}
}

func TestPutGetDelete(t *testing.T) {
	h := testHandler()
	vcf := "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ada\r\nUID:ada-1\r\nEND:VCARD\r\n"
	path := "/dav/addressbooks/alice/default/ada.vcf"

	rr := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPut, path, strings.NewReader(vcf))
	req.Header.Set("X-User-Id", "alice")
	req.Header.Set("Content-Type", "text/vcard")
	h.ServeHTTP(rr, req)
	if rr.Code != http.StatusCreated {
		t.Fatalf("put status %d", rr.Code)
	}
	etag := rr.Header().Get("ETag")
	if etag == "" {
		t.Fatal("missing etag")
	}

	rr2 := httptest.NewRecorder()
	req2 := httptest.NewRequest(http.MethodGet, path, nil)
	req2.Header.Set("X-User-Id", "alice")
	h.ServeHTTP(rr2, req2)
	if rr2.Code != 200 {
		t.Fatalf("get %d", rr2.Code)
	}
	got, _ := io.ReadAll(rr2.Body)
	if !strings.Contains(string(got), "FN:Ada") {
		t.Fatalf("body %s", got)
	}

	// Stale If-Match
	rr3 := httptest.NewRecorder()
	req3 := httptest.NewRequest(http.MethodPut, path, strings.NewReader(vcf))
	req3.Header.Set("X-User-Id", "alice")
	req3.Header.Set("If-Match", `"stale"`)
	h.ServeHTTP(rr3, req3)
	if rr3.Code != http.StatusPreconditionFailed {
		t.Fatalf("if-match status %d", rr3.Code)
	}

	// Propfind depth 1 on book
	rr4 := httptest.NewRecorder()
	req4 := httptest.NewRequest("PROPFIND", "/dav/addressbooks/alice/default/", nil)
	req4.Header.Set("X-User-Id", "alice")
	req4.Header.Set("Depth", "1")
	h.ServeHTTP(rr4, req4)
	if rr4.Code != 207 {
		t.Fatalf("propfind book %d", rr4.Code)
	}
	if !strings.Contains(rr4.Body.String(), "ada.vcf") {
		t.Fatalf("missing card in propfind: %s", rr4.Body.String())
	}

	// REPORT multiget
	rr5 := httptest.NewRecorder()
	reportBody := `<?xml version="1.0"?>
<card:addressbook-multiget xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <d:prop><d:getetag/><card:address-data/></d:prop>
  <d:href>/dav/addressbooks/alice/default/ada.vcf</d:href>
</card:addressbook-multiget>`
	req5 := httptest.NewRequest("REPORT", "/dav/addressbooks/alice/default/", strings.NewReader(reportBody))
	req5.Header.Set("X-User-Id", "alice")
	h.ServeHTTP(rr5, req5)
	if rr5.Code != 207 {
		t.Fatalf("report %d %s", rr5.Code, rr5.Body.String())
	}
	if !strings.Contains(rr5.Body.String(), "FN:Ada") {
		t.Fatalf("report missing data: %s", rr5.Body.String())
	}

	// Delete
	rr6 := httptest.NewRecorder()
	req6 := httptest.NewRequest(http.MethodDelete, path, nil)
	req6.Header.Set("X-User-Id", "alice")
	h.ServeHTTP(rr6, req6)
	if rr6.Code != http.StatusNoContent {
		t.Fatalf("delete %d", rr6.Code)
	}

	rr7 := httptest.NewRecorder()
	req7 := httptest.NewRequest(http.MethodGet, path, nil)
	req7.Header.Set("X-User-Id", "alice")
	h.ServeHTTP(rr7, req7)
	if rr7.Code != 404 {
		t.Fatalf("get after delete %d", rr7.Code)
	}
}

func TestParseDAVPath(t *testing.T) {
	cases := []struct {
		in   string
		kind resKind
		uid  string
		book string
		card string
	}{
		{"/dav/", resRoot, "", "", ""},
		{"/dav/principals/u1/", resPrincipal, "u1", "", ""},
		{"/dav/addressbooks/u1/", resHome, "u1", "", ""},
		{"/dav/addressbooks/u1/default/", resBook, "u1", "default", ""},
		{"/dav/addressbooks/u1/default/x.vcf", resCard, "u1", "default", "x"},
	}
	for _, c := range cases {
		p := parseDAVPath(c.in)
		if p.Kind != c.kind || p.UID != c.uid || p.BookID != c.book || p.CardID != c.card {
			t.Fatalf("%s => %+v want kind=%v uid=%s book=%s card=%s", c.in, p, c.kind, c.uid, c.book, c.card)
		}
	}
}

func TestWellKnownRedirect(t *testing.T) {
	h := testHandler()
	rr := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/.well-known/carddav", nil)
	req.Header.Set("X-User-Id", "alice")
	h.ServeHTTP(rr, req)
	if rr.Code != http.StatusMovedPermanently {
		t.Fatalf("status %d", rr.Code)
	}
	if loc := rr.Header().Get("Location"); loc != "/dav/" {
		t.Fatalf("location %q", loc)
	}
}

func TestCardDAVCompatibilityAndFailureMatrix(t *testing.T) {
	h := testHandler()
	auth := func(method, target, body string) *httptest.ResponseRecorder {
		request := httptest.NewRequest(method, target, strings.NewReader(body))
		request.Header.Set("X-User-Id", "alice")
		recorder := httptest.NewRecorder()
		h.ServeHTTP(recorder, request)
		return recorder
	}
	allProps := `<d:propfind xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav" xmlns:cs="http://calendarserver.org/ns/"><d:prop><d:resourcetype/><d:displayname/><d:current-user-principal/><d:principal-URL/><card:addressbook-home-set/><cs:getctag/><card:supported-address-data/><d:getetag/><d:getcontenttype/></d:prop></d:propfind>`
	for _, target := range []string{"/dav/", "/dav/principals/alice/", "/dav/addressbooks/alice/", "/dav/addressbooks/alice/default/"} {
		request := httptest.NewRequest("PROPFIND", target, strings.NewReader(allProps))
		request.Header.Set("X-User-Id", "alice")
		request.Header.Set("Depth", "1")
		recorder := httptest.NewRecorder()
		h.ServeHTTP(recorder, request)
		if recorder.Code != http.StatusMultiStatus {
			t.Fatalf("PROPFIND %s status=%d body=%s", target, recorder.Code, recorder.Body.String())
		}
	}

	cardPath := "/dav/addressbooks/alice/default/matrix.vcf"
	vcf := "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Matrix ]]> Contact\r\nEND:VCARD\r\n"
	if got := auth(http.MethodPut, cardPath, "invalid").Code; got != http.StatusBadRequest {
		t.Fatalf("invalid card status=%d", got)
	}
	h.MaxCardBytes = 8
	if got := auth(http.MethodPut, cardPath, vcf).Code; got != http.StatusRequestEntityTooLarge {
		t.Fatalf("oversized card status=%d", got)
	}
	h.MaxCardBytes = 1 << 20
	if got := auth(http.MethodPut, cardPath, vcf).Code; got != http.StatusCreated {
		t.Fatalf("create status=%d", got)
	}
	if got := auth(http.MethodHead, cardPath, "").Code; got != http.StatusOK {
		t.Fatalf("HEAD status=%d", got)
	}
	if got := auth(http.MethodPut, cardPath, vcf).Code; got != http.StatusNoContent {
		t.Fatalf("update status=%d", got)
	}
	ifNone := httptest.NewRequest(http.MethodPut, cardPath, strings.NewReader(vcf))
	ifNone.Header.Set("X-User-Id", "alice")
	ifNone.Header.Set("If-None-Match", "*")
	ifNoneResponse := httptest.NewRecorder()
	h.ServeHTTP(ifNoneResponse, ifNone)
	if ifNoneResponse.Code != http.StatusPreconditionFailed {
		t.Fatalf("If-None-Match status=%d", ifNoneResponse.Code)
	}
	query := `<card:addressbook-query xmlns:card="urn:ietf:params:xml:ns:carddav" xmlns:d="DAV:"><d:prop><d:getetag/><card:address-data/></d:prop></card:addressbook-query>`
	for _, target := range []string{"/dav/addressbooks/alice/default/", "/dav/addressbooks/alice/"} {
		recorder := auth("REPORT", target, query)
		if recorder.Code != http.StatusMultiStatus || !strings.Contains(recorder.Body.String(), "Matrix") {
			t.Fatalf("REPORT %s status=%d body=%s", target, recorder.Code, recorder.Body.String())
		}
	}
	missingReport := auth("REPORT", "/dav/addressbooks/alice/default/", `<card:addressbook-multiget xmlns:card="urn:ietf:params:xml:ns:carddav" xmlns:d="DAV:"><d:href>/dav/addressbooks/alice/default/missing.vcf</d:href></card:addressbook-multiget>`)
	if missingReport.Code != http.StatusMultiStatus {
		t.Fatalf("missing multiget status=%d", missingReport.Code)
	}
	for _, tc := range []struct {
		method, target string
		want           int
	}{
		{http.MethodGet, "/dav/addressbooks/alice/default/", http.StatusMethodNotAllowed},
		{http.MethodPut, "/dav/addressbooks/alice/default/", http.StatusMethodNotAllowed},
		{http.MethodDelete, "/dav/addressbooks/alice/default/", http.StatusMethodNotAllowed},
		{"REPORT", cardPath, http.StatusMethodNotAllowed},
		{"PATCH", cardPath, http.StatusMethodNotAllowed},
		{http.MethodGet, "/dav/addressbooks/alice/default/missing.vcf", http.StatusNotFound},
		{http.MethodDelete, "/dav/addressbooks/alice/default/missing.vcf", http.StatusNotFound},
		{http.MethodPut, "/dav/addressbooks/alice/missing/card.vcf", http.StatusNotFound},
	} {
		if got := auth(tc.method, tc.target, "").Code; got != tc.want {
			t.Errorf("%s %s status=%d want=%d", tc.method, tc.target, got, tc.want)
		}
	}
}

func TestCardDAVBackendFailuresReturnServerErrors(t *testing.T) {
	backend := newMem()
	h := &Handler{Backend: backend, UserID: func(r *http.Request) string { return "alice" }}
	for _, tc := range []struct {
		fail, method, target, body string
	}{
		{"books", "PROPFIND", "/dav/addressbooks/alice/", ""},
		{"book", "PROPFIND", "/dav/addressbooks/alice/default/", ""},
		{"cards", "PROPFIND", "/dav/addressbooks/alice/default/", ""},
		{"get", http.MethodGet, "/dav/addressbooks/alice/default/card.vcf", ""},
		{"get", "PROPFIND", "/dav/addressbooks/alice/default/card.vcf", ""},
		{"book", http.MethodPut, "/dav/addressbooks/alice/default/card.vcf", "BEGIN:VCARD\r\nEND:VCARD\r\n"},
		{"put", http.MethodPut, "/dav/addressbooks/alice/default/card.vcf", "BEGIN:VCARD\r\nEND:VCARD\r\n"},
		{"delete", http.MethodDelete, "/dav/addressbooks/alice/default/card.vcf", ""},
		{"cards", "REPORT", "/dav/addressbooks/alice/default/", ""},
		{"books", "REPORT", "/dav/addressbooks/alice/", ""},
	} {
		backend.fail = tc.fail
		request := httptest.NewRequest(tc.method, tc.target, strings.NewReader(tc.body))
		if tc.method == "PROPFIND" {
			request.Header.Set("Depth", "1")
		}
		recorder := httptest.NewRecorder()
		h.ServeHTTP(recorder, request)
		if recorder.Code != http.StatusInternalServerError {
			t.Errorf("%s %s fail=%s status=%d", tc.method, tc.target, tc.fail, recorder.Code)
		}
	}
}
