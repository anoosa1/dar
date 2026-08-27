package contacts

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/carddav"
)

type contactsSearchSink struct {
	docs []apps.SearchDocument
	err  error
}

func (s *contactsSearchSink) AddSearchDocument(_ context.Context, doc apps.SearchDocument) error {
	if s.err != nil {
		return s.err
	}
	s.docs = append(s.docs, doc)
	return nil
}

func TestContactsCardDAVSearchRecipientsAndMeMatrix(t *testing.T) {
	withTempDataRoot(t)
	rr := postJSON(t, testApp.handleCreate, "alice", `{
		"first_name":"Ada","last_name":"Lovelace","org":"Analytical Engines",
		"notes":"Searchable pioneer","emails":[{"value":"ADA@example.test","types":["work"]}],
		"groups":["Scientists"]
	}`)
	if rr.Code != 200 {
		t.Fatalf("create contact: %d %s", rr.Code, rr.Body.String())
	}
	created := decodeBody[Contact](t, rr)
	backend := fsCardDAV{app: testApp}
	if got := backend.PrincipalDisplayName("alice"); got != "alice" {
		t.Fatalf("principal name = %q", got)
	}
	books, err := backend.ListBooks("alice")
	if err != nil || len(books) != 1 {
		t.Fatalf("books = %#v, %v", books, err)
	}
	meta, err := backend.BookMeta("alice", "default")
	if err != nil || meta.DisplayName == "" || meta.CTag == "" {
		t.Fatalf("book meta = %#v, %v", meta, err)
	}
	if _, err = backend.BookMeta("alice", "missing"); !errors.Is(err, carddav.ErrNotFound) {
		t.Fatalf("missing book error = %v", err)
	}
	cards, err := backend.ListCards("alice", "default")
	if err != nil || len(cards) != 1 || cards[0].ID != created.ID {
		t.Fatalf("cards = %#v, %v", cards, err)
	}
	data, etag, err := backend.GetCard("alice", "default", created.ID)
	if err != nil || etag == "" || !strings.Contains(string(data), "Ada") {
		t.Fatalf("card = %q etag=%q err=%v", data, etag, err)
	}
	if _, _, err = backend.GetCard("bob", "default", created.ID); !errors.Is(err, carddav.ErrNotFound) {
		t.Fatalf("cross-user card error = %v", err)
	}

	vcf := []byte("BEGIN:VCARD\r\nVERSION:4.0\r\nUID:grace\r\nFN:Grace Hopper\r\nN:Hopper;Grace;;;\r\nEMAIL:grace@example.test\r\nEND:VCARD\r\n")
	graceETag, wasCreated, err := backend.PutCard("alice", "default", "grace", vcf, "", "*")
	if err != nil || !wasCreated || graceETag == "" {
		t.Fatalf("create card: etag=%q created=%v err=%v", graceETag, wasCreated, err)
	}
	if _, _, err = backend.PutCard("alice", "default", "grace", vcf, "", "*"); !errors.Is(err, carddav.ErrPrecondition) {
		t.Fatalf("If-None-Match error = %v", err)
	}
	if _, _, err = backend.PutCard("alice", "default", "grace", vcf, `"wrong"`, ""); !errors.Is(err, carddav.ErrPrecondition) {
		t.Fatalf("wrong If-Match error = %v", err)
	}
	newETag, wasCreated, err := backend.PutCard("alice", "default", "grace", vcf, graceETag, "")
	if err != nil || wasCreated || newETag == "" {
		t.Fatalf("update card: etag=%q created=%v err=%v", newETag, wasCreated, err)
	}
	if _, _, err = backend.PutCard("alice", "default", "../", vcf, "", ""); !errors.Is(err, carddav.ErrInvalidCard) {
		t.Fatalf("invalid ID error = %v", err)
	}
	if _, _, err = backend.PutCard("alice", "default", "empty", nil, "", ""); !errors.Is(err, carddav.ErrInvalidCard) {
		t.Fatalf("empty card error = %v", err)
	}
	if _, _, err = backend.PutCard("alice", "default", "bad", []byte("bad"), "", ""); !errors.Is(err, carddav.ErrInvalidCard) {
		t.Fatalf("malformed card error = %v", err)
	}

	sink := &contactsSearchSink{}
	if err = testApp.IndexSearch(context.Background(), "alice", sink); err != nil {
		t.Fatal(err)
	}
	if len(sink.docs) != 2 || sink.docs[0].Title == "" {
		t.Fatalf("search documents = %#v", sink.docs)
	}
	sink.err = context.Canceled
	if err = testApp.IndexSearch(context.Background(), "alice", sink); !errors.Is(err, context.Canceled) {
		t.Fatalf("search sink error = %v", err)
	}
	if firstNonBlank("", "  ", "value") != "value" || firstNonBlank("", " ") != "" {
		t.Fatal("firstNonBlank boundary failed")
	}

	recipients, err := testApp.SuggestRecipients(context.Background(), "alice", "example", 50)
	if err != nil || len(recipients) != 2 {
		t.Fatalf("recipients = %#v, %v", recipients, err)
	}
	if recipients, err = testApp.SuggestRecipients(context.Background(), "bob", "", 0); err != nil || len(recipients) != 0 {
		t.Fatalf("cross-user recipients = %#v, %v", recipients, err)
	}
	searchRec := getJSON(t, testApp.handleSearch, "/api/contacts/search?q=scientists&limit=1", "alice")
	if searchRec.Code != http.StatusOK || !strings.Contains(searchRec.Body.String(), "Ada") {
		t.Fatalf("contact search: %d %s", searchRec.Code, searchRec.Body.String())
	}
	healthRec := httptest.NewRecorder()
	testApp.handleHealth(healthRec, httptest.NewRequest(http.MethodGet, "/health", nil))
	if healthRec.Code != http.StatusOK {
		t.Fatalf("health = %d", healthRec.Code)
	}

	name, err := testApp.UpsertMe(context.Background(), "alice", "Alice Example", "alice@example.test", auth.UserProfile{
		Phone: "555-0100", Organisation: "Dar", Biography: "Profile biography", Birthday: "2000-01-02",
	})
	if err != nil || name == "" {
		t.Fatalf("upsert me = %q, %v", name, err)
	}
	canceledCtx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err = testApp.UpsertMe(canceledCtx, "alice", "Ignored", "", auth.UserProfile{}); !errors.Is(err, context.Canceled) {
		t.Fatalf("canceled upsert error = %v", err)
	}

	if err = backend.DeleteCard("alice", "default", "grace"); err != nil {
		t.Fatal(err)
	}
	if err = backend.DeleteCard("alice", "default", "grace"); !errors.Is(err, carddav.ErrNotFound) {
		t.Fatalf("delete missing error = %v", err)
	}
	if !etagMatch(`W/"x", "y"`, `"y"`) || etagMatch(`"x"`, `"y"`) || unquoteETag(`"x"`) != "x" || unquoteETag("x") != "x" {
		t.Fatal("ETag matching boundary failed")
	}
}
