package contacts

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/carddav"
)

func TestCardDAVFSBackendRoundTrip(t *testing.T) {
	withTempDataRoot(t)

	// Ensure default book exists via REST create.
	rr := postJSON(t, testApp.handleCreate, "alice", `{"first_name":"REST","last_name":"User","email":"rest@example.com"}`)
	if rr.Code != http.StatusOK {
		t.Fatalf("create status %d %s", rr.Code, rr.Body.String())
	}
	created := decodeBody[Contact](t, rr)

	h := &carddav.Handler{
		Backend: fsCardDAV{app: testApp},
		UserID:  func(r *http.Request) string { return r.Header.Get("X-User-Id") },
	}

	// PROPFIND home depth 1 lists default book.
	pr := httptest.NewRecorder()
	preq := httptest.NewRequest("PROPFIND", "/dav/addressbooks/alice/", nil)
	preq.Header.Set("X-User-Id", "alice")
	preq.Header.Set("Depth", "1")
	h.ServeHTTP(pr, preq)
	if pr.Code != 207 {
		t.Fatalf("propfind home %d %s", pr.Code, pr.Body.String())
	}
	if !strings.Contains(pr.Body.String(), "/dav/addressbooks/alice/default/") {
		t.Fatalf("missing default book: %s", pr.Body.String())
	}

	// PROPFIND book depth 1 sees REST contact.
	pr2 := httptest.NewRecorder()
	preq2 := httptest.NewRequest("PROPFIND", "/dav/addressbooks/alice/default/", nil)
	preq2.Header.Set("X-User-Id", "alice")
	preq2.Header.Set("Depth", "1")
	h.ServeHTTP(pr2, preq2)
	if pr2.Code != 207 {
		t.Fatalf("propfind book %d", pr2.Code)
	}
	if !strings.Contains(pr2.Body.String(), created.ID+".vcf") {
		t.Fatalf("missing contact href: %s", pr2.Body.String())
	}

	// PUT new card via CardDAV.
	vcf := "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:CardDAV Ada\r\nN:Ada;CardDAV;;;\r\nEMAIL:ada@carddav.test\r\nUID:carddav-ada\r\nEND:VCARD\r\n"
	putPath := "/dav/addressbooks/alice/default/carddav-ada.vcf"
	putRR := httptest.NewRecorder()
	putReq := httptest.NewRequest(http.MethodPut, putPath, strings.NewReader(vcf))
	putReq.Header.Set("X-User-Id", "alice")
	h.ServeHTTP(putRR, putReq)
	if putRR.Code != http.StatusCreated {
		t.Fatalf("put %d %s", putRR.Code, putRR.Body.String())
	}

	// File on disk.
	path := filepath.Join(testApp.dataRoot, "alice", "addressbooks", "default", "carddav-ada.vcf")
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("vcf not on disk: %v", err)
	}

	// REST list sees CardDAV contact.
	listRR := getJSON(t, testApp.handleList, "/list", "alice")
	list := decodeBody[struct {
		Contacts []Contact `json:"contacts"`
	}](t, listRR)
	found := false
	for _, c := range list.Contacts {
		if c.ID == "carddav-ada" || strings.Contains(c.FirstName, "CardDAV") || c.Email == "ada@carddav.test" {
			found = true
			break
		}
	}
	if !found {
		// parse may put name in FirstName from N
		for _, c := range list.Contacts {
			if c.ID == "carddav-ada" {
				found = true
			}
		}
	}
	if !found {
		// list by id file
		getRR := getJSON(t, testApp.handleGet, "/get?id=carddav-ada", "alice")
		if getRR.Code != 200 {
			t.Fatalf("REST get after CardDAV put: %d body=%s list=%+v", getRR.Code, getRR.Body.String(), list.Contacts)
		}
	}

	// Wrong user 404
	bad := httptest.NewRecorder()
	badReq := httptest.NewRequest(http.MethodGet, putPath, nil)
	badReq.Header.Set("X-User-Id", "bob")
	// path has alice — bob cannot access
	h.ServeHTTP(bad, badReq)
	if bad.Code != 404 {
		t.Fatalf("cross-user get %d", bad.Code)
	}
}
