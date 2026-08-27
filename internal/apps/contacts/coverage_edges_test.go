package contacts

import (
	"bytes"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/safefile"
	"github.com/emersion/go-vcard"
)

func TestContactsUpdateEveryMergeFieldAndFailure(t *testing.T) {
	withTempDataRoot(t)
	created := decodeBody[Contact](t, postJSON(t, testApp.handleCreate, "alice", `{"first_name":"Ada","last_name":"Lovelace","email":"old@example.test","phone":"old","org":"Old"}`))

	for _, tc := range []struct {
		method string
		body   string
		status int
	}{
		{http.MethodGet, `{}`, http.StatusMethodNotAllowed},
		{http.MethodPost, `{`, http.StatusBadRequest},
		{http.MethodPost, `{}`, http.StatusBadRequest},
		{http.MethodPost, `{"id":"missing"}`, http.StatusNotFound},
	} {
		rec := postJSON(t, testApp.handleUpdate, "alice", tc.body)
		if tc.method != http.MethodPost {
			req := withTestUser(httptest.NewRequest(tc.method, "/update", strings.NewReader(tc.body)), "alice")
			rec = httptest.NewRecorder()
			testApp.handleUpdate(rec, req)
		}
		if rec.Code != tc.status {
			t.Fatalf("%s update status=%d want=%d body=%s", tc.method, rec.Code, tc.status, rec.Body.String())
		}
	}

	full := `{
		"id":"` + created.ID + `",
		"first_name":"Augusta","last_name":"King","middle_name":"Ada","prefix":"Lady","suffix":"I",
		"org":"Engines","title":"Countess","role":"Programmer","notes":"Notes","url":"https://example.test",
		"birthday":"1815-12-10","color":"#123456","favorite":true,
		"emails":[],"phones":[],"addresses":[],"urls":[],"groups":[]
	}`
	rec := postJSON(t, testApp.handleUpdate, "alice", full)
	if rec.Code != http.StatusOK {
		t.Fatalf("full update status=%d body=%s", rec.Code, rec.Body.String())
	}
	got := decodeBody[Contact](t, rec)
	if got.MiddleName != "Ada" || got.Prefix != "Lady" || got.Suffix != "I" || got.Role != "Programmer" ||
		got.Notes != "Notes" || got.URL != "https://example.test" || got.Birthday != "1815-12-10" ||
		!got.Favorite || got.Email != "" || got.Phone != "" || len(got.Emails) != 0 || len(got.Phones) != 0 ||
		len(got.Addresses) != 0 || len(got.Groups) != 0 {
		t.Fatalf("full merged contact=%+v", got)
	}

	legacy := `{"id":"` + created.ID + `","email":"new@example.test","phone":"+1 555"}`
	rec = postJSON(t, testApp.handleUpdate, "alice", legacy)
	got = decodeBody[Contact](t, rec)
	if got.Email != "new@example.test" || len(got.Emails) != 1 || got.Phone != "+1 555" || len(got.Phones) != 1 {
		t.Fatalf("legacy merged contact=%+v", got)
	}
	clear := `{"id":"` + created.ID + `","email":"","phone":""}`
	rec = postJSON(t, testApp.handleUpdate, "alice", clear)
	got = decodeBody[Contact](t, rec)
	if got.Email != "" || len(got.Emails) != 0 || got.Phone != "" || len(got.Phones) != 0 {
		t.Fatalf("legacy cleared contact=%+v", got)
	}
}

func TestContactsHandlerMethodValidationAndClosedDatabase(t *testing.T) {
	app := preferencesTestApp(t)
	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		target  string
		body    string
	}{
		{app.handleList, http.MethodPost, "/api/contacts/list", ""},
		{app.handleGet, http.MethodPost, "/api/contacts/get", ""},
		{app.handleCreate, http.MethodGet, "/api/contacts/create", ""},
		{app.handleUpdate, http.MethodGet, "/api/contacts/update", ""},
		{app.handleDelete, http.MethodGet, "/api/contacts/delete", ""},
		{app.handleSearch, http.MethodPost, "/api/contacts/search", ""},
		{app.handleExport, http.MethodPost, "/api/contacts/export", ""},
		{app.handleImport, http.MethodGet, "/api/contacts/import", ""},
		{app.handleBooks, http.MethodPatch, "/api/contacts/books", ""},
		{app.handleBooksRename, http.MethodGet, "/api/contacts/books/rename", ""},
		{app.handleBooksDelete, http.MethodGet, "/api/contacts/books/delete", ""},
		{app.handleBooksVisibility, http.MethodGet, "/api/contacts/books/visibility", ""},
		{app.handlePreferences, http.MethodDelete, "/api/contacts/preferences", ""},
		{app.handleCreate, http.MethodPost, "/api/contacts/create", `{`},
		{app.handleUpdate, http.MethodPost, "/api/contacts/update", `{`},
		{app.handleDelete, http.MethodPost, "/api/contacts/delete", `{`},
		{app.handleBooks, http.MethodPost, "/api/contacts/books", `{`},
		{app.handleBooksRename, http.MethodPost, "/api/contacts/books/rename", `{`},
		{app.handleBooksDelete, http.MethodPost, "/api/contacts/books/delete", `{`},
		{app.handleBooksVisibility, http.MethodPost, "/api/contacts/books/visibility", `{`},
		{app.handlePreferences, http.MethodPut, "/api/contacts/preferences", `{`},
	} {
		req := withTestUser(httptest.NewRequest(tc.method, tc.target, strings.NewReader(tc.body)), "alice")
		rec := httptest.NewRecorder()
		tc.handler(rec, req)
		if rec.Code < 200 || rec.Code >= 600 {
			t.Errorf("%s %s returned invalid status %d: %s", tc.method, tc.target, rec.Code, rec.Body.String())
		}
	}
	if err := app.db.Close(); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		target  string
		body    string
	}{
		{app.handleBooks, http.MethodGet, "/api/contacts/books", ""},
		{app.handleBooks, http.MethodPost, "/api/contacts/books", `{"name":"Closed"}`},
		{app.handleBooksRename, http.MethodPost, "/api/contacts/books/rename", `{"id":"personal","name":"Closed"}`},
		{app.handleBooksDelete, http.MethodPost, "/api/contacts/books/delete", `{"id":"personal"}`},
		{app.handleBooksVisibility, http.MethodPost, "/api/contacts/books/visibility", `{"id":"personal","visible":false}`},
		{app.handlePreferences, http.MethodGet, "/api/contacts/preferences", ""},
		{app.handlePreferences, http.MethodPut, "/api/contacts/preferences", `{"sort":"display_name"}`},
	} {
		req := withTestUser(httptest.NewRequest(tc.method, tc.target, strings.NewReader(tc.body)), "alice")
		rec := httptest.NewRecorder()
		tc.handler(rec, req)
		if rec.Code < 200 || rec.Code >= 600 {
			t.Errorf("closed DB %s returned invalid status %d: %s", tc.target, rec.Code, rec.Body.String())
		}
	}
}

func TestContactsLegacyMigrationBookAndVCardHelperEdges(t *testing.T) {
	app := preferencesTestApp(t)
	legacy := filepath.Join(app.dataRoot, "alice", "contacts")
	if err := os.MkdirAll(filepath.Join(legacy, "directory.vcf"), 0o700); err != nil {
		t.Fatal(err)
	}
	for name, body := range map[string]string{
		"migrate.vcf": "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Migrated\r\nEND:VCARD\r\n",
		"ignore.txt":  "ignored",
	} {
		if err := os.WriteFile(filepath.Join(legacy, name), []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	destination := app.contactsDir("alice")
	if _, err := os.Stat(filepath.Join(destination, "migrate.vcf")); err != nil {
		t.Fatalf("legacy contact not migrated: %v", err)
	}
	if _, err := os.Stat(filepath.Join(legacy, "migrate.vcf")); !os.IsNotExist(err) {
		t.Fatalf("legacy source still exists: %v", err)
	}
	if got := app.contactsDir("alice"); got != destination {
		t.Fatalf("contactsDir=%q want %q", got, destination)
	}
	if got := app.defaultBookName("my-work"); got != "My Work" {
		t.Fatalf("default book name=%q", got)
	}
	if got := app.slugifyBookName("default"); got != "book-default" {
		t.Fatalf("default slug=%q", got)
	}
	if got := app.slugifyBookName("!!!"); !strings.HasPrefix(got, "book-") {
		t.Fatalf("punctuation slug=%q", got)
	}
	if count := app.countVCards(filepath.Join(app.dataRoot, "missing")); count != 0 {
		t.Fatalf("missing vCard count=%d", count)
	}
	if got := app.fieldTypes(nil); got != nil {
		t.Fatalf("nil field types=%v", got)
	}
	explicit := &vcard.Field{Params: vcard.Params{"TYPE": {"HOME", "VOICE"}}}
	if got := app.fieldTypes(explicit); len(got) != 2 {
		t.Fatalf("explicit types=%v", got)
	}
	legacyField := &vcard.Field{Params: vcard.Params{"HOME": {""}, "CELL": {""}, "X-CUSTOM": {""}}}
	if got := app.fieldTypes(legacyField); len(got) != 2 {
		t.Fatalf("legacy types=%v", got)
	}
}

func TestContactsRESTBookMeAndPaginationValidationMatrix(t *testing.T) {
	withTempDataRoot(t)
	call := func(handler http.HandlerFunc, method, target, body string) *httptest.ResponseRecorder {
		t.Helper()
		request := withTestUser(httptest.NewRequest(method, target, strings.NewReader(body)), "alice")
		recorder := httptest.NewRecorder()
		handler(recorder, request)
		return recorder
	}
	for _, tc := range []struct {
		name    string
		handler http.HandlerFunc
		method  string
		target  string
		body    string
		want    int
	}{
		{"get missing", testApp.handleGet, http.MethodGet, "/api/contacts/get?id=missing", ``, http.StatusNotFound},
		{"create method", testApp.handleCreate, http.MethodGet, "/api/contacts/create", ``, http.StatusMethodNotAllowed},
		{"create malformed", testApp.handleCreate, http.MethodPost, "/api/contacts/create", `{`, http.StatusBadRequest},
		{"delete method", testApp.handleDelete, http.MethodGet, "/api/contacts/delete", ``, http.StatusMethodNotAllowed},
		{"delete missing", testApp.handleDelete, http.MethodPost, "/api/contacts/delete?id=missing", ``, http.StatusNotFound},
		{"export missing", testApp.handleExport, http.MethodGet, "/api/contacts/export?id=missing", ``, http.StatusNotFound},
		{"import method", testApp.handleImport, http.MethodGet, "/api/contacts/import", ``, http.StatusMethodNotAllowed},
		{"import no multipart", testApp.handleImport, http.MethodPost, "/api/contacts/import", ``, http.StatusBadRequest},
		{"book method", testApp.handleBooks, http.MethodPatch, "/api/contacts/books", ``, http.StatusMethodNotAllowed},
		{"book malformed", testApp.handleBooks, http.MethodPost, "/api/contacts/books", `{`, http.StatusBadRequest},
		{"book blank", testApp.handleBooks, http.MethodPost, "/api/contacts/books", `{"name":" "}`, http.StatusBadRequest},
		{"rename method", testApp.handleBooksRename, http.MethodGet, "/api/contacts/books/rename", ``, http.StatusMethodNotAllowed},
		{"rename malformed", testApp.handleBooksRename, http.MethodPost, "/api/contacts/books/rename", `{`, http.StatusBadRequest},
		{"rename missing fields", testApp.handleBooksRename, http.MethodPost, "/api/contacts/books/rename", `{}`, http.StatusBadRequest},
		{"rename missing book", testApp.handleBooksRename, http.MethodPost, "/api/contacts/books/rename", `{"id":"missing","name":"Missing"}`, http.StatusNotFound},
		{"delete book method", testApp.handleBooksDelete, http.MethodGet, "/api/contacts/books/delete", ``, http.StatusMethodNotAllowed},
		{"delete book malformed", testApp.handleBooksDelete, http.MethodPost, "/api/contacts/books/delete", `{`, http.StatusBadRequest},
		{"delete default", testApp.handleBooksDelete, http.MethodPost, "/api/contacts/books/delete", `{"id":"default"}`, http.StatusBadRequest},
		{"delete missing book", testApp.handleBooksDelete, http.MethodPost, "/api/contacts/books/delete", `{"id":"missing"}`, http.StatusNotFound},
		{"visibility method", testApp.handleBooksVisibility, http.MethodGet, "/api/contacts/books/visibility", ``, http.StatusMethodNotAllowed},
		{"visibility malformed", testApp.handleBooksVisibility, http.MethodPost, "/api/contacts/books/visibility", `{`, http.StatusBadRequest},
		{"visibility missing", testApp.handleBooksVisibility, http.MethodPost, "/api/contacts/books/visibility", `{"id":"missing","visible":true}`, http.StatusNotFound},
		{"me method", testApp.handleMe, http.MethodGet, "/api/contacts/me", ``, http.StatusMethodNotAllowed},
		{"me malformed", testApp.handleMe, http.MethodPost, "/api/contacts/me", `{`, http.StatusBadRequest},
		{"me missing name", testApp.handleMe, http.MethodPost, "/api/contacts/me", `{}`, http.StatusBadRequest},
	} {
		t.Run(tc.name, func(t *testing.T) {
			recorder := call(tc.handler, tc.method, tc.target, tc.body)
			if recorder.Code != tc.want {
				t.Fatalf("status=%d want=%d body=%s", recorder.Code, tc.want, recorder.Body.String())
			}
		})
	}

	book := call(testApp.handleBooks, http.MethodPost, "/api/contacts/books", `{"name":"Project Team"}`)
	if book.Code != http.StatusOK || !strings.Contains(book.Body.String(), `"visible":true`) {
		t.Fatalf("create book status=%d body=%s", book.Code, book.Body.String())
	}
	renamed := call(testApp.handleBooksRename, http.MethodPost, "/api/contacts/books/rename", `{"id":"project-team","name":"Project Archive"}`)
	if renamed.Code != http.StatusOK || !strings.Contains(renamed.Body.String(), "Project Archive") {
		t.Fatalf("rename book status=%d body=%s", renamed.Code, renamed.Body.String())
	}
	for _, visible := range []bool{false, true} {
		body := `{"id":"project-team","visible":` + map[bool]string{true: "true", false: "false"}[visible] + `}`
		response := call(testApp.handleBooksVisibility, http.MethodPost, "/api/contacts/books/visibility", body)
		if response.Code != http.StatusOK {
			t.Fatalf("visibility %v status=%d body=%s", visible, response.Code, response.Body.String())
		}
	}

	for _, body := range []string{
		`{"first_name":"Ada","last_name":"Lovelace","notes":"pagination needle","book_id":"project-team"}`,
		`{"org":"Analytical Engines","book_id":"project-team","groups":["Scientists"]}`,
	} {
		if response := call(testApp.handleCreate, http.MethodPost, "/api/contacts/create", body); response.Code != http.StatusOK {
			t.Fatalf("create contact status=%d body=%s", response.Code, response.Body.String())
		}
	}
	list := call(testApp.handleList, http.MethodGet, "/api/contacts/list?book=project-team&limit=500&offset=-2", ``)
	if list.Code != http.StatusOK || !strings.Contains(list.Body.String(), "Ada") {
		t.Fatalf("list status=%d body=%s", list.Code, list.Body.String())
	}
	empty := call(testApp.handleList, http.MethodGet, "/api/contacts/list?book=project-team&offset=999", ``)
	if empty.Code != http.StatusOK || !strings.Contains(empty.Body.String(), `"contacts":[]`) {
		t.Fatalf("empty page status=%d body=%s", empty.Code, empty.Body.String())
	}
	search := call(testApp.handleSearch, http.MethodGet, "/api/contacts/search?book=project-team&q=needle&limit=500&offset=-1", ``)
	if search.Code != http.StatusOK || !strings.Contains(search.Body.String(), "Ada") {
		t.Fatalf("search status=%d body=%s", search.Code, search.Body.String())
	}

	meBody := `{"display_name":"Alice Example","email":"alice@example.test","phone":"555-0100","organisation":"Dar"}`
	for range 2 {
		response := call(testApp.handleMe, http.MethodPost, "/api/contacts/me", meBody)
		if response.Code != http.StatusOK || !strings.Contains(response.Body.String(), `"is_me":true`) {
			t.Fatalf("upsert me status=%d body=%s", response.Code, response.Body.String())
		}
	}
	deleted := call(testApp.handleBooksDelete, http.MethodPost, "/api/contacts/books/delete", `{"id":"project-team"}`)
	if deleted.Code != http.StatusOK {
		t.Fatalf("delete book status=%d body=%s", deleted.Code, deleted.Body.String())
	}
}

func TestContactsFilesystemBookMetadataNamingAndLookupMatrix(t *testing.T) {
	anonymous := httptest.NewRecorder()
	handlePage(anonymous, httptest.NewRequest(http.MethodGet, "/contacts", nil))
	if anonymous.Code != http.StatusSeeOther {
		t.Fatalf("anonymous page status=%d", anonymous.Code)
	}
	app := New(Config{DataRoot: t.TempDir()})
	for _, tc := range []struct {
		contact Contact
		want    string
	}{
		{Contact{Org: "Organisation"}, "Organisation"},
		{Contact{Email: "legacy@example.test"}, "legacy@example.test"},
		{Contact{Emails: []TypedValue{{Value: "first@example.test"}}}, "first@example.test"},
		{Contact{}, "Unnamed"},
	} {
		if got := tc.contact.DisplayName(); got != tc.want {
			t.Errorf("display name=%q want=%q", got, tc.want)
		}
	}
	if (Contact{LastName: "Lovelace", FirstName: "Ada"}).preferenceSortKey("last_name") != "lovelace\x00ada" {
		t.Fatal("last-name sort key regression")
	}
	if (Contact{FirstName: "Ada", LastName: "Lovelace"}).preferenceSortKey("first_name") != "ada\x00lovelace" {
		t.Fatal("first-name sort key regression")
	}
	for input, want := range map[string]string{"": "default", "...": "default", "a..b": "ab", " Work! ": "Work"} {
		if got := app.sanitizeBookID(input); got != want {
			t.Errorf("sanitizeBookID(%q)=%q want=%q", input, got, want)
		}
	}
	if app.defaultBookName("my--work") != "My  Work" {
		t.Fatalf("default book title=%q", app.defaultBookName("my--work"))
	}
	if app.countVCards(filepath.Join(t.TempDir(), "missing")) != 0 {
		t.Fatal("missing address book had contacts")
	}
	if !app.bookVisible(bookMeta{}) {
		t.Fatal("nil visibility should be visible")
	}
	hidden := false
	if app.bookVisible(bookMeta{Visible: &hidden}) {
		t.Fatal("false visibility ignored")
	}

	bookDir := filepath.Join(app.booksRoot("alice"), "work")
	if err := os.MkdirAll(bookDir, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(bookDir, "meta.json"), []byte(`{"name":""}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := app.readBookMeta(bookDir, "work"); got.Name != "Work" {
		t.Fatalf("empty metadata name=%+v", got)
	}
	if err := os.WriteFile(filepath.Join(bookDir, "meta.json"), []byte("{"), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := app.readBookMeta(bookDir, "work"); got.Name != "Work" {
		t.Fatalf("corrupt metadata fallback=%+v", got)
	}
	if err := app.writeBookMeta(bookDir, bookMeta{}); err != nil {
		t.Fatal(err)
	}
	if got := app.readBookMeta(bookDir, "work"); got.Name != "Address book" {
		t.Fatalf("default metadata name=%+v", got)
	}

	if got := app.slugifyBookName("Default"); got != "book-default" {
		t.Fatalf("default slug=%q", got)
	}
	if got := app.slugifyBookName("!!!"); !strings.HasPrefix(got, "book-") {
		t.Fatalf("punctuation slug=%q", got)
	}
	if got := app.slugifyBookName(" My__Work "); got != "my-work" {
		t.Fatalf("normalized slug=%q", got)
	}
	if got := app.uniqueBookID("alice", "work"); got != "work-2" {
		t.Fatalf("colliding book ID=%q", got)
	}

	app.bookDir("alice", "default")
	if err := os.MkdirAll(filepath.Join(app.booksRoot("alice"), "invalid name"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(app.booksRoot("alice"), "stray"), []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	ids := app.listBookDirs("alice")
	for _, id := range ids {
		if id == "invalid name" {
			t.Fatalf("invalid directory included: %v", ids)
		}
	}
	contact := Contact{ID: "lookup", FirstName: "Lookup", BookID: "work"}
	if err := safefile.Write(filepath.Join(bookDir, contact.ID+".vcf"), []byte(app.toVCard(contact))); err != nil {
		t.Fatal(err)
	}
	if _, book, err := app.findContactPath("alice", contact.ID, "work"); err != nil || book != "work" {
		t.Fatalf("preferred lookup book=%q err=%v", book, err)
	}
	if _, _, err := app.findContactPath("alice", contact.ID, "missing"); !os.IsNotExist(err) {
		t.Fatalf("missing preferred lookup err=%v", err)
	}
	if _, book, err := app.findContactPath("alice", contact.ID, ""); err != nil || book != "work" {
		t.Fatalf("scanned lookup book=%q err=%v", book, err)
	}
}

func TestContactsSingleExportImportFallbackAndBlankMatrix(t *testing.T) {
	app := New(Config{DataRoot: t.TempDir()})
	bookDir := app.bookDir("alice", "default")
	card := "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:single\r\nFN:Single Export\r\nEMAIL:single@example.test\r\nEND:VCARD"
	if err := os.WriteFile(filepath.Join(bookDir, "single.vcf"), []byte(card), 0o600); err != nil {
		t.Fatal(err)
	}
	callExport := func(target string) *httptest.ResponseRecorder {
		req := withTestUser(httptest.NewRequest(http.MethodGet, target, nil), "alice")
		rec := httptest.NewRecorder()
		app.handleExport(rec, req)
		return rec
	}
	single := callExport("/api/contacts/export?id=single")
	if single.Code != http.StatusOK || !strings.Contains(single.Body.String(), "Single Export") ||
		!strings.Contains(single.Header().Get("Content-Disposition"), "single.vcf") {
		t.Fatalf("single export status=%d headers=%v body=%s", single.Code, single.Header(), single.Body.String())
	}
	all := callExport("/api/contacts/export")
	if all.Code != http.StatusOK || !strings.HasSuffix(all.Body.String(), "\n") {
		t.Fatalf("all export status=%d body=%q", all.Code, all.Body.String())
	}
	filtered := callExport("/api/contacts/export?book=default")
	if filtered.Code != http.StatusOK || !strings.Contains(filtered.Header().Get("Content-Disposition"), "default.vcf") {
		t.Fatalf("filtered export status=%d headers=%v", filtered.Code, filtered.Header())
	}
	missing := callExport("/api/contacts/export?id=missing")
	if missing.Code != http.StatusNotFound {
		t.Fatalf("missing export status=%d", missing.Code)
	}

	importCard := func(raw string) *httptest.ResponseRecorder {
		t.Helper()
		var body bytes.Buffer
		writer := multipart.NewWriter(&body)
		part, err := writer.CreateFormFile("file", "contacts.vcf")
		if err != nil {
			t.Fatal(err)
		}
		_, _ = part.Write([]byte(raw))
		_ = writer.Close()
		req := withTestUser(httptest.NewRequest(http.MethodPost, "/api/contacts/import?book=default", &body), "alice")
		req.Header.Set("Content-Type", writer.FormDataContentType())
		rec := httptest.NewRecorder()
		app.handleImport(rec, req)
		return rec
	}
	fallback := importCard("garbage before card\r\nBEGIN:vCard\r\nVERSION:3.0\r\nFN:Fallback Person\r\nEMAIL:fallback@example.test\r\nEND:vCard\r\n")
	if fallback.Code != http.StatusOK || !strings.Contains(fallback.Body.String(), `"imported":1`) {
		t.Fatalf("fallback import status=%d body=%s", fallback.Code, fallback.Body.String())
	}
	for _, raw := range []string{
		"BEGIN:VCARD\r\nVERSION:3.0\r\nEND:VCARD\r\n",
		"BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Never Closed\r\n",
		"not a vcard",
	} {
		rec := importCard(raw)
		if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"imported":0`) {
			t.Errorf("empty/broken import status=%d body=%s", rec.Code, rec.Body.String())
		}
	}
	for _, contact := range []Contact{
		{FirstName: "name"}, {Email: "email@example.test"}, {Emails: []TypedValue{{Value: "typed@example.test"}}},
	} {
		if app.isBlankContact(contact) {
			t.Errorf("nonblank contact classified blank: %+v", contact)
		}
	}
	if !app.isBlankContact(Contact{}) || app.meDisplayName("") != "Me" ||
		app.meDisplayName("Alice (Me)") != "Alice (Me)" || app.meDisplayName("Alice") != "Alice (Me)" {
		t.Fatal("blank/me-contact helper regression")
	}
}
