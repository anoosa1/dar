package contacts

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/server"
)

var testApp = New(Config{})

func withTestUser(r *http.Request, id string) *http.Request {
	return r.WithContext(context.WithValue(r.Context(), server.UserContextKey, &auth.User{ID: id}))
}

func withTempDataRoot(t *testing.T) string {
	t.Helper()
	old := testApp.dataRoot
	dir := t.TempDir()
	testApp.dataRoot = dir
	t.Cleanup(func() { testApp.dataRoot = old })
	return dir
}

func postJSON(t *testing.T, handler http.HandlerFunc, uid string, body string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(http.MethodPost, "/", bytes.NewBufferString(body))
	req.Header.Set("Content-Type", "application/json")
	req = withTestUser(req, uid)
	rr := httptest.NewRecorder()
	handler(rr, req)
	return rr
}

func getJSON(t *testing.T, handler http.HandlerFunc, target string, uid string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(http.MethodGet, target, nil)
	req = withTestUser(req, uid)
	rr := httptest.NewRecorder()
	handler(rr, req)
	return rr
}

func decodeBody[T any](t *testing.T, rr *httptest.ResponseRecorder) T {
	t.Helper()
	var got T
	if err := json.NewDecoder(rr.Body).Decode(&got); err != nil {
		t.Fatalf("decode response: %v\nbody: %s", err, rr.Body.String())
	}
	return got
}

func TestCreateWritesVCardForRequestUser(t *testing.T) {
	dir := withTempDataRoot(t)

	rr := postJSON(t, testApp.handleCreate, "alice", `{
		"first_name":"Ada","last_name":"Lovelace","prefix":"Ms.","suffix":"PhD",
		"emails":[{"value":"ada@example.com","types":["work"]},{"value":"ada@home.org","types":["home"]}],
		"phones":[{"value":"555-0100","types":["work","voice"]},{"value":"555-0200","types":["cell"]}],
		"addresses":[
			{"types":["home"],"street":"123 Main St","locality":"London","postal_code":"SW1","country":"UK"},
			{"types":["work"],"street":"456 Corp Ave","locality":"New York","postal_code":"10001","country":"USA"}
		],
		"org":"Analytical Engines","title":"Chief Scientist","role":"Inventor",
		"notes":"First programmer","url":"https://ada.example.com","birthday":"18151210"
	}`)
	if rr.Code != http.StatusOK {
		t.Fatalf("create status = %d, want %d; body=%s", rr.Code, http.StatusOK, rr.Body.String())
	}
	created := decodeBody[Contact](t, rr)
	if created.ID == "" {
		t.Fatal("created contact ID is empty")
	}
	path := filepath.Join(dir, "alice", "addressbooks", "default", created.ID+".vcf")
	parsed, err := testApp.parseVCard(path)
	if err != nil {
		t.Fatalf("parse created vCard: %v", err)
	}
	if parsed.FirstName != "Ada" || parsed.LastName != "Lovelace" || parsed.Prefix != "Ms." || parsed.Suffix != "PhD" {
		t.Fatalf("name fields: %+v", parsed)
	}
	if len(parsed.Emails) != 2 || parsed.Emails[0].Value != "ada@example.com" {
		t.Fatalf("emails: %+v", parsed.Emails)
	}
	if len(parsed.Phones) != 2 {
		t.Fatalf("phones: %+v", parsed.Phones)
	}
	if len(parsed.Addresses) != 2 || parsed.Addresses[0].Street != "123 Main St" {
		t.Fatalf("addresses: %+v", parsed.Addresses)
	}
	if parsed.Org != "Analytical Engines" || parsed.Title != "Chief Scientist" || parsed.Role != "Inventor" {
		t.Fatalf("org fields: %+v", parsed)
	}
	if parsed.Notes != "First programmer" || parsed.URL != "https://ada.example.com" || parsed.Birthday != "18151210" {
		t.Fatalf("misc fields: notes=%q url=%q bday=%q", parsed.Notes, parsed.URL, parsed.Birthday)
	}
}

func TestListReturnsContactsForSameUserOnly(t *testing.T) {
	withTempDataRoot(t)

	postJSON(t, testApp.handleCreate, "alice", `{"first_name":"Alice","emails":[{"value":"alice@example.com"}]}`)
	postJSON(t, testApp.handleCreate, "bob", `{"first_name":"Bob","emails":[{"value":"bob@example.com"}]}`)

	rr := getJSON(t, testApp.handleList, "/list", "alice")
	if rr.Code != http.StatusOK {
		t.Fatalf("list status = %d, want %d", rr.Code, http.StatusOK)
	}
	got := decodeBody[struct {
		Contacts []Contact `json:"contacts"`
	}](t, rr)
	if len(got.Contacts) == 0 || got.Contacts[0].Color != contactsTheme.Pink {
		t.Fatalf("first generated contact color = %+v, want %q", got.Contacts, contactsTheme.Pink)
	}
	if len(got.Contacts) != 1 {
		t.Fatalf("contacts len = %d, want 1: %+v", len(got.Contacts), got.Contacts)
	}
	if got.Contacts[0].FirstName != "Alice" {
		t.Fatalf("listed contact = %+v, want Alice only", got.Contacts[0])
	}
}

func TestUpdateChangesExistingContact(t *testing.T) {
	withTempDataRoot(t)

	createdRR := postJSON(t, testApp.handleCreate, "alice", `{"first_name":"Ada","emails":[{"value":"ada@example.com"}]}`)
	created := decodeBody[Contact](t, createdRR)

	updateBody := `{"id":"` + created.ID + `","first_name":"Augusta","last_name":"King","emails":[{"value":"augusta@example.com","types":["work"]}],"phones":[{"value":"555-0199","types":["cell"]}],"org":"Mathematics","title":"Countess"}`
	rr := postJSON(t, testApp.handleUpdate, "alice", updateBody)
	if rr.Code != http.StatusOK {
		t.Fatalf("update status = %d, want %d; body=%s", rr.Code, http.StatusOK, rr.Body.String())
	}

	getRR := getJSON(t, testApp.handleGet, "/get?id="+created.ID, "alice")
	got := decodeBody[Contact](t, getRR)
	if got.FirstName != "Augusta" || got.LastName != "King" {
		t.Fatalf("updated name = %+v", got)
	}
	if len(got.Emails) != 1 || got.Emails[0].Value != "augusta@example.com" {
		t.Fatalf("updated emails = %+v", got.Emails)
	}
	if got.Org != "Mathematics" || got.Title != "Countess" {
		t.Fatalf("updated org = %+v", got)
	}
}

func TestDeleteRemovesContact(t *testing.T) {
	withTempDataRoot(t)

	createdRR := postJSON(t, testApp.handleCreate, "alice", `{"first_name":"Ada","emails":[{"value":"ada@example.com"}]}`)
	created := decodeBody[Contact](t, createdRR)

	req := httptest.NewRequest(http.MethodPost, "/delete?id="+created.ID, nil)
	req = withTestUser(req, "alice")
	rr := httptest.NewRecorder()
	testApp.handleDelete(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("delete status = %d, want %d", rr.Code, http.StatusOK)
	}

	getRR := getJSON(t, testApp.handleGet, "/get?id="+created.ID, "alice")
	if getRR.Code != http.StatusNotFound {
		t.Fatalf("get after delete status = %d, want %d", getRR.Code, http.StatusNotFound)
	}
}

func TestExportReturnsVCardFile(t *testing.T) {
	withTempDataRoot(t)

	postJSON(t, testApp.handleCreate, "alice", `{"first_name":"Ada","last_name":"Lovelace","emails":[{"value":"ada@example.com"}]}`)
	postJSON(t, testApp.handleCreate, "alice", `{"first_name":"Bob","emails":[{"value":"bob@example.com"}]}`)

	rr := getJSON(t, testApp.handleExport, "/export", "alice")
	if rr.Code != http.StatusOK {
		t.Fatalf("export status = %d, want %d", rr.Code, http.StatusOK)
	}
	body := rr.Body.String()
	if !strings.Contains(body, "BEGIN:VCARD") || !strings.Contains(body, "END:VCARD") {
		t.Fatal("export output missing VCARD markers")
	}
	if !strings.Contains(body, "Ada") || !strings.Contains(body, "Bob") {
		t.Fatalf("export output missing contact data: %s", body)
	}
	if hdr := rr.Header().Get("Content-Disposition"); !strings.Contains(hdr, "attachment") {
		t.Fatalf("Content-Disposition missing attachment: %s", hdr)
	}
}

func TestExportEmptyUserReturnsEmptyBody(t *testing.T) {
	withTempDataRoot(t)
	rr := getJSON(t, testApp.handleExport, "/export", "emptyuser")
	if rr.Code != http.StatusOK {
		t.Fatalf("export status = %d, want %d", rr.Code, http.StatusOK)
	}
}

func TestImportParsesMultiCardVCF(t *testing.T) {
	dir := withTempDataRoot(t)

	vcf := "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ada Lovelace\r\nN:Lovelace;Ada;;;\r\nEMAIL;TYPE=work:ada@example.com\r\nTEL;TYPE=cell:555-0100\r\nEND:VCARD\r\n" +
		"BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Bob Smith\r\nN:Smith;Bob;;;\r\nEMAIL;TYPE=home:bob@example.com\r\nTEL;TYPE=work,voice:555-0000\r\nEND:VCARD\r\n"

	body := strings.NewReader(
		"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"contacts.vcf\"\r\nContent-Type: text/vcard\r\n\r\n" +
			vcf + "\r\n--boundary--\r\n",
	)
	req := httptest.NewRequest(http.MethodPost, "/import", body)
	req.Header.Set("Content-Type", "multipart/form-data; boundary=boundary")
	req = withTestUser(req, "alice")
	rr := httptest.NewRecorder()
	testApp.handleImport(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("import status = %d, want %d; body=%s", rr.Code, http.StatusOK, rr.Body.String())
	}
	got := decodeBody[struct {
		Imported int `json:"imported"`
	}](t, rr)
	if got.Imported != 2 {
		t.Fatalf("imported = %d, want 2", got.Imported)
	}

	listRR := getJSON(t, testApp.handleList, "/list", "alice")
	list := decodeBody[struct {
		Contacts []Contact `json:"contacts"`
	}](t, listRR)
	if len(list.Contacts) != 2 {
		t.Fatalf("contacts len after import = %d, want 2", len(list.Contacts))
	}
	os.RemoveAll(dir)
}

func TestImportRejectsNonMultipart(t *testing.T) {
	withTempDataRoot(t)
	req := httptest.NewRequest(http.MethodPost, "/import", strings.NewReader("not multipart"))
	req = withTestUser(req, "alice")
	rr := httptest.NewRecorder()
	testApp.handleImport(rr, req)
	if rr.Code != 400 {
		t.Fatalf("import status = %d, want 400; body=%s", rr.Code, rr.Body.String())
	}
}

func TestImportSkipsBlankCards(t *testing.T) {
	withTempDataRoot(t)
	vcf := "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:\r\nEND:VCARD\r\n"
	body := strings.NewReader(
		"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"blank.vcf\"\r\nContent-Type: text/vcard\r\n\r\n" +
			vcf + "\r\n--boundary--\r\n",
	)
	req := httptest.NewRequest(http.MethodPost, "/import", body)
	req.Header.Set("Content-Type", "multipart/form-data; boundary=boundary")
	req = withTestUser(req, "alice")
	rr := httptest.NewRecorder()
	testApp.handleImport(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("import status = %d, want %d; body=%s", rr.Code, http.StatusOK, rr.Body.String())
	}
	got := decodeBody[struct {
		Imported int `json:"imported"`
	}](t, rr)
	if got.Imported != 0 {
		t.Fatalf("imported = %d, want 0", got.Imported)
	}
}

func TestVCardRoundtripPreservesTypes(t *testing.T) {
	withTempDataRoot(t)

	rr := postJSON(t, testApp.handleCreate, "alice", `{
		"first_name":"Test","last_name":"User",
		"emails":[{"value":"test@work.com","types":["work"]},{"value":"test@home.org","types":["home"]}],
		"phones":[{"value":"111","types":["cell"]}]
	}`)
	created := decodeBody[Contact](t, rr)

	// Re-read and verify types preserved
	getRR := getJSON(t, testApp.handleGet, "/get?id="+created.ID, "alice")
	got := decodeBody[Contact](t, getRR)
	if len(got.Emails) != 2 {
		t.Fatalf("emails count = %d, want 2", len(got.Emails))
	}
	if len(got.Emails[0].Types) == 0 {
		t.Fatal("email types not preserved")
	}
	if len(got.Phones) != 1 || len(got.Phones[0].Types) == 0 {
		t.Fatal("phone types not preserved")
	}
}

func TestOrgOnlyContact(t *testing.T) {
	withTempDataRoot(t)

	rr := postJSON(t, testApp.handleCreate, "alice", `{"org":"Acme Corp","title":"CEO","emails":[{"value":"ceo@acme.com","types":["work"]}]}`)
	c := decodeBody[Contact](t, rr)
	if c.DisplayName() != "Acme Corp" {
		t.Fatalf("DisplayName = %q, want Acme Corp", c.DisplayName())
	}
	if c.SortKey() != "acme corp" {
		t.Fatalf("SortKey = %q", c.SortKey())
	}
}

func TestImportWithoutVersion(t *testing.T) {
	withTempDataRoot(t)

	// vCard without VERSION field — many exports omit it
	vcf := "BEGIN:VCARD\r\nFN:NoVersion User\r\nN:User;NoVersion;;;\r\nEMAIL:noversion@example.com\r\nTEL:555-9999\r\nORG:NoVersionCo\r\nEND:VCARD\r\n"

	body := strings.NewReader(
		"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"noversion.vcf\"\r\nContent-Type: text/vcard\r\n\r\n" +
			vcf + "\r\n--boundary--\r\n",
	)
	req := httptest.NewRequest(http.MethodPost, "/import", body)
	req.Header.Set("Content-Type", "multipart/form-data; boundary=boundary")
	req = withTestUser(req, "alice")
	rr := httptest.NewRecorder()
	testApp.handleImport(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("import status = %d, want %d; body=%s", rr.Code, http.StatusOK, rr.Body.String())
	}
	got := decodeBody[struct {
		Imported int `json:"imported"`
	}](t, rr)
	if got.Imported != 1 {
		t.Fatalf("imported = %d, want 1", got.Imported)
	}

	// Verify the imported contact has all fields
	listRR := getJSON(t, testApp.handleList, "/list", "alice")
	list := decodeBody[struct {
		Contacts []Contact `json:"contacts"`
	}](t, listRR)
	if len(list.Contacts) != 1 {
		t.Fatalf("contacts len = %d, want 1", len(list.Contacts))
	}
	c := list.Contacts[0]
	if c.FirstName != "NoVersion" || c.LastName != "User" {
		t.Fatalf("name: %+v", c)
	}
	if len(c.Emails) != 1 || c.Emails[0].Value != "noversion@example.com" {
		t.Fatalf("emails: %+v", c.Emails)
	}
	if len(c.Phones) != 1 || c.Phones[0].Value != "555-9999" {
		t.Fatalf("phones: %+v", c.Phones)
	}
	if c.Org != "NoVersionCo" {
		t.Fatalf("org = %q", c.Org)
	}
}

func TestParseDAVx5SampleMultiFields(t *testing.T) {
	// Real-world multi-value card (like data/default/addressbooks samples).
	dir := withTempDataRoot(t)
	vcf := `BEGIN:VCARD
VERSION:4.0
UID:5f0c891d-43e6-4cbd-aac0-a19b2420b342
FN:Anas Sherif
N:Sherif;Anas;;;
TEL;TYPE=cell:+1-905-749-6219
EMAIL;TYPE=home:ibnshaher5@gmail.com
EMAIL;TYPE=work:anas@asherif.xyz
EMAIL:anas@waifu.club
URL;TYPE=x-homepage;VALUE=URI:https://asherif.xyz
ADR;TYPE=home:;;373 Pringle Ave;Milton;ON;L9T 7M5;Canada
BDAY:20060806
REV:20260615T075131Z
END:VCARD
`
	bookDir := filepath.Join(dir, "u1", "addressbooks", "default")
	os.MkdirAll(bookDir, 0755)
	path := filepath.Join(bookDir, "sample.vcf")
	if err := os.WriteFile(path, []byte(vcf), 0644); err != nil {
		t.Fatal(err)
	}
	c, err := testApp.parseVCard(path)
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	if c.FirstName != "Anas" || c.LastName != "Sherif" {
		t.Fatalf("name: %+v", c)
	}
	if len(c.Emails) != 3 {
		t.Fatalf("emails: %+v", c.Emails)
	}
	if len(c.Phones) != 1 || c.Phones[0].Value != "+1-905-749-6219" {
		t.Fatalf("phones: %+v", c.Phones)
	}
	if len(c.Addresses) != 1 || c.Addresses[0].Street != "373 Pringle Ave" {
		t.Fatalf("addresses: %+v", c.Addresses)
	}
	if c.Birthday != "20060806" {
		t.Fatalf("birthday: %q", c.Birthday)
	}
	if c.URL != "https://asherif.xyz" && (len(c.URLs) == 0 || c.URLs[0].Value != "https://asherif.xyz") {
		t.Fatalf("url: %q urls=%+v", c.URL, c.URLs)
	}
	// Round-trip
	out := testApp.toVCard(c)
	tmp := filepath.Join(bookDir, "roundtrip.vcf")
	os.WriteFile(tmp, []byte(out), 0644)
	c2, err := testApp.parseVCard(tmp)
	if err != nil {
		t.Fatalf("reparse: %v", err)
	}
	if len(c2.Emails) != 3 || len(c2.Phones) != 1 {
		t.Fatalf("roundtrip emails/phones: %+v / %+v", c2.Emails, c2.Phones)
	}
}

func TestParseVCard21Address(t *testing.T) {
	dir := withTempDataRoot(t)

	// vCard 2.1 format: ADR;HOME:;;street;city;region;zip;country
	vcf := "BEGIN:VCARD\r\nVERSION:2.1\r\nFN:Test\r\nADR;HOME:;;123 Home St;London;ON;SW1;UK\r\nADR;WORK:;;456 Work Ave;Toronto;ON;M5V;CA\r\nEND:VCARD\r\n"

	bookDir := filepath.Join(dir, "testuser", "addressbooks", "default")
	os.MkdirAll(bookDir, 0755)
	os.WriteFile(filepath.Join(bookDir, "test.vcf"), []byte(vcf), 0644)

	c, err := testApp.parseVCard(filepath.Join(bookDir, "test.vcf"))
	if err != nil {
		t.Fatalf("parseVCard: %v", err)
	}
	if len(c.Addresses) != 2 {
		t.Fatalf("addresses = %d, want 2: %+v", len(c.Addresses), c.Addresses)
	}
	if c.Addresses[0].Street != "123 Home St" || c.Addresses[0].Locality != "London" {
		t.Fatalf("home address: %+v", c.Addresses[0])
	}
	if len(c.Addresses[0].Types) == 0 || c.Addresses[0].Types[0] != "home" {
		t.Fatalf("home address types: %v", c.Addresses[0].Types)
	}
	if c.Addresses[1].Street != "456 Work Ave" {
		t.Fatalf("work address: %+v", c.Addresses[1])
	}
}

func TestBooksCRUD(t *testing.T) {
	dir := withTempDataRoot(t)

	// List starts with default only.
	rr := getJSON(t, testApp.handleBooks, "/books", "alice")
	if rr.Code != http.StatusOK {
		t.Fatalf("list books status = %d; body=%s", rr.Code, rr.Body.String())
	}
	got := decodeBody[struct {
		Books []AddressBook `json:"books"`
	}](t, rr)
	if len(got.Books) != 1 || got.Books[0].ID != "default" {
		t.Fatalf("initial books = %+v, want [default]", got.Books)
	}

	// Create Work book.
	createRR := postJSON(t, testApp.handleBooks, "alice", `{"name":"Work"}`)
	if createRR.Code != http.StatusOK {
		t.Fatalf("create book status = %d; body=%s", createRR.Code, createRR.Body.String())
	}
	created := decodeBody[AddressBook](t, createRR)
	if created.ID == "" || created.Name != "Work" || !created.Visible {
		t.Fatalf("created book = %+v", created)
	}
	metaPath := filepath.Join(dir, "alice", "addressbooks", created.ID, "meta.json")
	if _, err := os.Stat(metaPath); err != nil {
		t.Fatalf("meta.json missing: %v", err)
	}

	// Create contact in work book.
	cRR := postJSON(t, testApp.handleCreate, "alice", `{"first_name":"W","book_id":"`+created.ID+`","emails":[{"value":"w@example.com"}]}`)
	if cRR.Code != http.StatusOK {
		t.Fatalf("create contact status = %d; body=%s", cRR.Code, cRR.Body.String())
	}
	c := decodeBody[Contact](t, cRR)
	if c.BookID != created.ID {
		t.Fatalf("contact book_id = %q, want %q", c.BookID, created.ID)
	}

	// Contact also in default.
	postJSON(t, testApp.handleCreate, "alice", `{"first_name":"D","emails":[{"value":"d@example.com"}]}`)

	// List all visible → both.
	listRR := getJSON(t, testApp.handleList, "/list", "alice")
	list := decodeBody[struct {
		Contacts []Contact `json:"contacts"`
	}](t, listRR)
	if len(list.Contacts) != 2 {
		t.Fatalf("list all len = %d, want 2", len(list.Contacts))
	}

	// List by book.
	listWork := getJSON(t, testApp.handleList, "/list?book="+created.ID, "alice")
	workList := decodeBody[struct {
		Contacts []Contact `json:"contacts"`
	}](t, listWork)
	if len(workList.Contacts) != 1 || workList.Contacts[0].FirstName != "W" {
		t.Fatalf("list work = %+v", workList.Contacts)
	}

	// Contact count on books.
	booksRR := getJSON(t, testApp.handleBooks, "/books", "alice")
	books := decodeBody[struct {
		Books []AddressBook `json:"books"`
	}](t, booksRR)
	var defCount, workCount int
	for _, b := range books.Books {
		if b.ID == "default" {
			defCount = b.ContactCount
		}
		if b.ID == created.ID {
			workCount = b.ContactCount
		}
	}
	if defCount != 1 || workCount != 1 {
		t.Fatalf("counts default=%d work=%d, want 1/1; books=%+v", defCount, workCount, books.Books)
	}

	// Export by book.
	exp := getJSON(t, testApp.handleExport, "/export?book="+created.ID, "alice")
	if exp.Code != http.StatusOK {
		t.Fatalf("export book status = %d", exp.Code)
	}
	if !strings.Contains(exp.Body.String(), "W") || strings.Contains(exp.Body.String(), "FN:D") {
		// D contact may or may not appear as FN:D — check first name in card
	}
	if !strings.Contains(exp.Body.String(), "W") {
		t.Fatalf("export work missing W: %s", exp.Body.String())
	}
	// Default-only contact should not be in work export.
	if strings.Count(exp.Body.String(), "BEGIN:VCARD") != 1 {
		t.Fatalf("export work card count = %d, want 1; body=%s", strings.Count(exp.Body.String(), "BEGIN:VCARD"), exp.Body.String())
	}

	// Rename.
	ren := postJSON(t, testApp.handleBooksRename, "alice", `{"id":"`+created.ID+`","name":"Office"}`)
	if ren.Code != http.StatusOK {
		t.Fatalf("rename status = %d; body=%s", ren.Code, ren.Body.String())
	}
	renamed := decodeBody[AddressBook](t, ren)
	if renamed.Name != "Office" {
		t.Fatalf("renamed = %+v", renamed)
	}

	// Hide work book → only default contact remains.
	vis := postJSON(t, testApp.handleBooksVisibility, "alice", `{"id":"`+created.ID+`","visible":false}`)
	if vis.Code != http.StatusOK {
		t.Fatalf("visibility status = %d; body=%s", vis.Code, vis.Body.String())
	}
	listHidden := getJSON(t, testApp.handleList, "/list", "alice")
	hiddenList := decodeBody[struct {
		Contacts []Contact `json:"contacts"`
	}](t, listHidden)
	if len(hiddenList.Contacts) != 1 || hiddenList.Contacts[0].FirstName != "D" {
		t.Fatalf("list after hide = %+v, want only D", hiddenList.Contacts)
	}

	// Show work again, hide default → only work contact.
	postJSON(t, testApp.handleBooksVisibility, "alice", `{"id":"`+created.ID+`","visible":true}`)
	hideDef := postJSON(t, testApp.handleBooksVisibility, "alice", `{"id":"default","visible":false}`)
	if hideDef.Code != http.StatusOK {
		t.Fatalf("hide default status = %d; body=%s", hideDef.Code, hideDef.Body.String())
	}
	listDefHidden := getJSON(t, testApp.handleList, "/list", "alice")
	defHidden := decodeBody[struct {
		Contacts []Contact `json:"contacts"`
	}](t, listDefHidden)
	if len(defHidden.Contacts) != 1 || defHidden.Contacts[0].FirstName != "W" {
		t.Fatalf("list after hide default = %+v, want only W", defHidden.Contacts)
	}

	// Hide both books → empty list (no fallback to default).
	postJSON(t, testApp.handleBooksVisibility, "alice", `{"id":"`+created.ID+`","visible":false}`)
	listNone := getJSON(t, testApp.handleList, "/list", "alice")
	none := decodeBody[struct {
		Contacts []Contact `json:"contacts"`
	}](t, listNone)
	if len(none.Contacts) != 0 {
		t.Fatalf("list with all books hidden = %+v, want empty", none.Contacts)
	}

	// Restore default visibility for remaining tests.
	postJSON(t, testApp.handleBooksVisibility, "alice", `{"id":"default","visible":true}`)

	// Cannot delete default.
	delDef := postJSON(t, testApp.handleBooksDelete, "alice", `{"id":"default"}`)
	if delDef.Code != 400 {
		t.Fatalf("delete default status = %d, want 400", delDef.Code)
	}

	// Delete work book.
	del := postJSON(t, testApp.handleBooksDelete, "alice", `{"id":"`+created.ID+`"}`)
	if del.Code != http.StatusOK {
		t.Fatalf("delete book status = %d; body=%s", del.Code, del.Body.String())
	}
	if _, err := os.Stat(filepath.Join(dir, "alice", "addressbooks", created.ID)); !os.IsNotExist(err) {
		t.Fatalf("book dir still exists after delete")
	}
	final := getJSON(t, testApp.handleBooks, "/books", "alice")
	finalBooks := decodeBody[struct {
		Books []AddressBook `json:"books"`
	}](t, final)
	if len(finalBooks.Books) != 1 || finalBooks.Books[0].ID != "default" {
		t.Fatalf("books after delete = %+v", finalBooks.Books)
	}
}

func TestBooksCreateRequiresName(t *testing.T) {
	withTempDataRoot(t)
	rr := postJSON(t, testApp.handleBooks, "alice", `{}`)
	if rr.Code != 400 {
		t.Fatalf("status = %d, want 400", rr.Code)
	}
}

func TestMeContactUpsert(t *testing.T) {
	withTempDataRoot(t)

	rr := postJSON(t, testApp.handleMe, "alice", `{
		"display_name":"Ada",
		"email":"ada@example.com",
		"phone":"+1 555 0100",
		"address":"1 Computing Lane",
		"website":"https://ada.example",
		"twitter":"@ada",
		"fediverse":"@ada@example.social",
		"organisation":"Analytical Engines",
		"role":"Mathematician",
		"headline":"Poetical Science",
		"biography":"First programmer",
		"location":"London",
		"birthday":"1815-12-10"
	}`)
	if rr.Code != http.StatusOK {
		t.Fatalf("me create status = %d; body=%s", rr.Code, rr.Body.String())
	}
	created := decodeBody[Contact](t, rr)
	if created.FirstName != "Ada (Me)" {
		t.Fatalf("name = %q", created.FirstName)
	}
	if !created.IsMe {
		t.Fatal("expected is_me")
	}
	if created.Email != "ada@example.com" {
		t.Fatalf("email = %q", created.Email)
	}
	if created.Phone != "+1 555 0100" {
		t.Fatalf("phone = %q", created.Phone)
	}
	if created.Org != "Analytical Engines" {
		t.Fatalf("org = %q", created.Org)
	}
	if created.Title != "Poetical Science" {
		t.Fatalf("title/headline = %q", created.Title)
	}
	if created.Role != "Mathematician" {
		t.Fatalf("role = %q", created.Role)
	}
	if created.Notes != "First programmer" {
		t.Fatalf("notes/biography = %q", created.Notes)
	}
	if created.Birthday != "1815-12-10" {
		t.Fatalf("birthday = %q", created.Birthday)
	}
	if created.URL != "https://ada.example" {
		t.Fatalf("url = %q", created.URL)
	}
	if len(created.Addresses) == 0 || created.Addresses[0].Street != "1 Computing Lane" || created.Addresses[0].Locality != "London" {
		t.Fatalf("addresses = %+v", created.Addresses)
	}
	// Website + twitter + fediverse URLs
	if len(created.URLs) < 2 {
		t.Fatalf("urls = %+v, want website + social", created.URLs)
	}
	id := created.ID

	// Second save updates same contact (full personal info refresh).
	rr2 := postJSON(t, testApp.handleMe, "alice", `{
		"display_name":"Ada Lovelace",
		"email":"ada@example.com",
		"phone":"+1 555 9999",
		"organisation":"Royal Society",
		"role":"Countess",
		"headline":"",
		"biography":"Updated bio",
		"birthday":"1815-12-10"
	}`)
	if rr2.Code != http.StatusOK {
		t.Fatalf("me update status = %d; body=%s", rr2.Code, rr2.Body.String())
	}
	updated := decodeBody[Contact](t, rr2)
	if updated.ID != id {
		t.Fatalf("id changed: %q vs %q", id, updated.ID)
	}
	if updated.FirstName != "Ada Lovelace (Me)" {
		t.Fatalf("updated name = %q", updated.FirstName)
	}
	if updated.Phone != "+1 555 9999" {
		t.Fatalf("updated phone = %q", updated.Phone)
	}
	if updated.Org != "Royal Society" || updated.Role != "Countess" {
		t.Fatalf("updated org/role = %q / %q", updated.Org, updated.Role)
	}
	if updated.Notes != "Updated bio" {
		t.Fatalf("updated notes = %q", updated.Notes)
	}
	// Cleared fields
	if updated.Title != "" {
		t.Fatalf("headline should be cleared, got %q", updated.Title)
	}
	if len(updated.Addresses) != 0 {
		t.Fatalf("address should be cleared, got %+v", updated.Addresses)
	}

	// No duplicate me contacts.
	listRR := getJSON(t, testApp.handleList, "/list", "alice")
	list := decodeBody[struct {
		Contacts []Contact `json:"contacts"`
	}](t, listRR)
	meCount := 0
	for _, c := range list.Contacts {
		if c.IsMe {
			meCount++
		}
	}
	if meCount != 1 {
		t.Fatalf("me contacts = %d, want 1", meCount)
	}
}

func TestContactIDRejectsPathTraversal(t *testing.T) {
	for _, id := range []string{"../secret", `..\\secret`, ".", "..", "contact\r\nX-Test: injected"} {
		if validContactID(id) {
			t.Fatalf("unsafe contact id %q was accepted", id)
		}
	}
}
