package caldav

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/server"
)

type memoryStore struct {
	collections []Collection
	objects     map[string]Object
	err         error
	createErr   error
	putErr      error
	deleteErr   error
	created     []string
	deleted     []string
}

func (store *memoryStore) Collections(string) ([]Collection, error) {
	if store.err != nil {
		return nil, store.err
	}
	return append([]Collection(nil), store.collections...), nil
}

func (store *memoryStore) CreateCollection(_ string, name string) (Collection, error) {
	if store.createErr != nil {
		return Collection{}, store.createErr
	}
	created := Collection{ID: "created-" + name, Name: name}
	store.collections = append(store.collections, created)
	store.created = append(store.created, name)
	return created, nil
}

func (store *memoryStore) Objects(_ string, collectionID string) ([]Object, error) {
	if store.err != nil {
		return nil, store.err
	}
	var result []Object
	for _, object := range store.objects {
		if strings.HasPrefix(object.Href, collectionID+"/") {
			result = append(result, object)
		}
	}
	return result, nil
}

func (store *memoryStore) Object(_ string, collectionID, href string) (Object, error) {
	if store.err != nil {
		return Object{}, store.err
	}
	object, ok := store.objects[collectionID+"/"+href]
	if !ok {
		return Object{}, errors.New("not found")
	}
	return object, nil
}

func (store *memoryStore) PutObject(_ string, collectionID, href, data string) (Object, error) {
	if store.putErr != nil {
		return Object{}, store.putErr
	}
	object := Object{Href: collectionID + "/" + href, Data: data, ETag: `"new-etag"`}
	if store.objects == nil {
		store.objects = map[string]Object{}
	}
	store.objects[object.Href] = object
	return object, nil
}

func (store *memoryStore) DeleteObject(_ string, collectionID, href string) error {
	if store.deleteErr != nil {
		return store.deleteErr
	}
	key := collectionID + "/" + href
	if _, ok := store.objects[key]; !ok {
		return errors.New("not found")
	}
	delete(store.objects, key)
	store.deleted = append(store.deleted, key)
	return nil
}

func calDAVRequest(method, target, body string, authenticated bool) *http.Request {
	req := httptest.NewRequest(method, target, strings.NewReader(body))
	if authenticated {
		user := &auth.User{ID: "alice", Username: "alice"}
		req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, user))
	}
	return req
}

func serveCalDAV(service *Service, method, target, body string, authenticated bool) *httptest.ResponseRecorder {
	recorder := httptest.NewRecorder()
	service.Handler(recorder, calDAVRequest(method, target, body, authenticated))
	return recorder
}

func TestServiceHomeAuthenticationAndCollectionDeduplication(t *testing.T) {
	service := New()
	unauthorized := serveCalDAV(service, http.MethodGet, "/remote.php/dav/calendars", "", false)
	if unauthorized.Code != http.StatusUnauthorized || unauthorized.Header().Get("WWW-Authenticate") == "" {
		t.Fatalf("unauthorized response = %d %#v", unauthorized.Code, unauthorized.Header())
	}

	service.Register(&memoryStore{collections: []Collection{{ID: "work", Name: `<Work & "Play">`}}})
	service.Register(&memoryStore{collections: []Collection{{ID: "work", Name: "duplicate"}, {ID: "tasks", Name: "Tasks"}}})
	for _, target := range []string{
		"/remote.php/dav",
		"/remote.php/dav/calendars",
		"/remote.php/dav/calendars/alice",
		"/caldav",
	} {
		response := serveCalDAV(service, http.MethodGet, target, "", true)
		if response.Code != http.StatusMultiStatus {
			t.Fatalf("%s status = %d body=%s", target, response.Code, response.Body.String())
		}
		body := response.Body.String()
		if strings.Count(body, "/work/") != 1 || !strings.Contains(body, "/tasks/") {
			t.Errorf("%s collections body=%s", target, body)
		}
		if strings.Contains(body, `<Work & "Play">`) || !strings.Contains(body, "&lt;Work &amp; &#34;Play&#34;&gt;") {
			t.Errorf("display name was not XML escaped: %s", body)
		}
		if response.Header().Get("DAV") != "1, 2, calendar-access" {
			t.Errorf("DAV header = %q", response.Header().Get("DAV"))
		}
	}
}

func TestServiceCollectionMethodsAndFailures(t *testing.T) {
	store := &memoryStore{
		collections: []Collection{{ID: "work", Name: "Work"}},
		objects: map[string]Object{
			"work/event.ics": {Href: "work/event.ics", Data: "BEGIN:VCALENDAR&END:VCALENDAR", ETag: `"one&two"`},
		},
	}
	service := New()
	service.Register(store)

	options := serveCalDAV(service, http.MethodOptions, "/remote.php/dav/calendars/alice/work", "", true)
	if options.Code != http.StatusOK || !strings.Contains(options.Header().Get("Allow"), "REPORT") {
		t.Fatalf("OPTIONS = %d %#v", options.Code, options.Header())
	}
	propfind := serveCalDAV(service, "PROPFIND", "/remote.php/dav/calendars/alice/work", "", true)
	if propfind.Code != http.StatusMultiStatus || !strings.Contains(propfind.Body.String(), "event.ics") || !strings.Contains(propfind.Body.String(), "&#34;one&amp;two&#34;") {
		t.Fatalf("PROPFIND = %d %s", propfind.Code, propfind.Body.String())
	}
	report := serveCalDAV(service, "REPORT", "/remote.php/dav/calendars/alice/work", `<calendar-query/>`, true)
	if report.Code != http.StatusMultiStatus || !strings.Contains(report.Body.String(), "BEGIN:VCALENDAR&amp;END:VCALENDAR") {
		t.Fatalf("REPORT = %d %s", report.Code, report.Body.String())
	}
	unsupported := serveCalDAV(service, http.MethodPost, "/remote.php/dav/calendars/alice/work", "", true)
	if unsupported.Code != http.StatusMethodNotAllowed {
		t.Fatalf("unsupported status = %d", unsupported.Code)
	}
	missing := serveCalDAV(service, "PROPFIND", "/remote.php/dav/calendars/alice/missing", "", true)
	if missing.Code != http.StatusNotFound {
		t.Fatalf("missing status = %d", missing.Code)
	}

	created := serveCalDAV(service, "MKCALENDAR", "/remote.php/dav/calendars/alice/personal", "", true)
	if created.Code != http.StatusCreated || created.Header().Get("Location") != "/remote.php/dav/calendars/alice/created-personal/" || len(store.created) != 1 {
		t.Fatalf("MKCALENDAR = %d %#v created=%v", created.Code, created.Header(), store.created)
	}

	store.createErr = errors.New("invalid calendar")
	failedCreate := serveCalDAV(service, "MKCALENDAR", "/remote.php/dav/calendars/alice/bad", "", true)
	if failedCreate.Code != http.StatusBadRequest {
		t.Fatalf("failed create status = %d", failedCreate.Code)
	}
	empty := New()
	noStore := serveCalDAV(empty, "MKCALENDAR", "/remote.php/dav/calendars/alice/new", "", true)
	if noStore.Code != http.StatusNotFound {
		t.Fatalf("empty MKCALENDAR status = %d", noStore.Code)
	}
}

func TestServiceObjectMethodsAndFailures(t *testing.T) {
	store := &memoryStore{
		collections: []Collection{{ID: "work", Name: "Work"}},
		objects: map[string]Object{
			"work/old.ics": {Href: "work/old.ics", Data: "old", ETag: `"old"`},
		},
	}
	service := New()
	service.Register(store)

	get := serveCalDAV(service, http.MethodGet, "/remote.php/dav/calendars/alice/work/old.ics", "", true)
	if get.Code != http.StatusOK || get.Body.String() != "old" || get.Header().Get("ETag") != `"old"` {
		t.Fatalf("GET = %d %#v %q", get.Code, get.Header(), get.Body.String())
	}
	put := serveCalDAV(service, http.MethodPut, "/remote.php/dav/calendars/alice/work/new.ics", "new", true)
	if put.Code != http.StatusCreated || put.Header().Get("ETag") != `"new-etag"` || store.objects["work/new.ics"].Data != "new" {
		t.Fatalf("PUT = %d %#v objects=%#v", put.Code, put.Header(), store.objects)
	}
	deleteResponse := serveCalDAV(service, http.MethodDelete, "/remote.php/dav/calendars/alice/work/new.ics", "", true)
	if deleteResponse.Code != http.StatusNoContent || len(store.deleted) != 1 {
		t.Fatalf("DELETE = %d deleted=%v", deleteResponse.Code, store.deleted)
	}

	for _, tc := range []struct {
		name   string
		method string
		target string
		status int
	}{
		{"wrong extension", http.MethodGet, "/remote.php/dav/calendars/alice/work/a.txt", http.StatusBadRequest},
		{"unknown collection", http.MethodGet, "/remote.php/dav/calendars/alice/missing/a.ics", http.StatusNotFound},
		{"missing object", http.MethodGet, "/remote.php/dav/calendars/alice/work/missing.ics", http.StatusNotFound},
		{"missing delete", http.MethodDelete, "/remote.php/dav/calendars/alice/work/missing.ics", http.StatusNotFound},
		{"unsupported object method", http.MethodPost, "/remote.php/dav/calendars/alice/work/old.ics", http.StatusMethodNotAllowed},
	} {
		t.Run(tc.name, func(t *testing.T) {
			response := serveCalDAV(service, tc.method, tc.target, "", true)
			if response.Code != tc.status {
				t.Fatalf("status = %d, want %d body=%s", response.Code, tc.status, response.Body.String())
			}
		})
	}

	store.putErr = errors.New("invalid calendar data")
	failedPut := serveCalDAV(service, http.MethodPut, "/remote.php/dav/calendars/alice/work/bad.ics", "bad", true)
	if failedPut.Code != http.StatusBadRequest {
		t.Fatalf("failed PUT status = %d", failedPut.Code)
	}
	store.deleteErr = errors.New("storage failed")
	failedDelete := serveCalDAV(service, http.MethodDelete, "/remote.php/dav/calendars/alice/work/old.ics", "", true)
	if failedDelete.Code != http.StatusNotFound {
		t.Fatalf("failed DELETE status = %d", failedDelete.Code)
	}
}

func TestServiceStorageErrors(t *testing.T) {
	broken := &memoryStore{collections: []Collection{{ID: "work", Name: "Work"}}, err: errors.New("broken")}
	service := New()
	service.Register(broken)

	home := serveCalDAV(service, http.MethodGet, "/remote.php/dav/calendars", "", true)
	if home.Code != http.StatusInternalServerError {
		t.Fatalf("home storage error status = %d", home.Code)
	}

	// A store whose collection enumeration fails is ignored during lookup.
	collection := serveCalDAV(service, "PROPFIND", "/remote.php/dav/calendars/alice/work", "", true)
	if collection.Code != http.StatusNotFound {
		t.Fatalf("collection lookup error status = %d", collection.Code)
	}

	workingLookup := &memoryStore{collections: []Collection{{ID: "work", Name: "Work"}}, err: nil}
	service = New()
	service.Register(workingLookup)
	workingLookup.err = errors.New("objects unavailable")
	// Make lookup succeed once by using a separate store that owns the collection
	// and reports failures only from Objects.
	owner := &objectErrorStore{memoryStore: memoryStore{collections: []Collection{{ID: "work", Name: "Work"}}}}
	service = New()
	service.Register(owner)
	for _, method := range []string{"PROPFIND", "REPORT"} {
		response := serveCalDAV(service, method, "/remote.php/dav/calendars/alice/work", "", true)
		if response.Code != http.StatusInternalServerError {
			t.Errorf("%s storage error status = %d", method, response.Code)
		}
	}
}

type objectErrorStore struct{ memoryStore }

func (store *objectErrorStore) Objects(string, string) ([]Object, error) {
	return nil, errors.New("objects unavailable")
}
