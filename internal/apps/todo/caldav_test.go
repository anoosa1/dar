package todo

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/caldav"
	"github.com/anoosa/dar/internal/server"
)

func davRequest(method, target, body, uid string) *http.Request {
	r := httptest.NewRequest(method, target, strings.NewReader(body))
	return r.WithContext(context.WithValue(r.Context(), server.UserContextKey, &auth.User{ID: uid, Email: uid + "@example.test"}))
}

func TestCalDAVVTODOUsesTodoStore(t *testing.T) {
	app := testApp(t)
	list, err := app.ensureDefaultList("alice")
	if err != nil {
		t.Fatal(err)
	}
	svc := caldav.New()
	svc.Register(app)
	ics := "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:ignored\r\nSUMMARY:DAV habit\r\nDUE:20260712T120000Z\r\nRRULE:FREQ=DAILY\r\nX-DAR-HABIT:TRUE\r\nEND:VTODO\r\nEND:VCALENDAR\r\n"
	base := "/remote.php/dav/calendars/alice/" + list.ID + "/from-dav.ics"
	put := httptest.NewRecorder()
	svc.Handler(put, davRequest(http.MethodPut, base, ics, "alice"))
	if put.Code != http.StatusCreated {
		t.Fatalf("PUT status=%d body=%s", put.Code, put.Body.String())
	}
	get := httptest.NewRecorder()
	svc.Handler(get, davRequest(http.MethodGet, base, "", "alice"))
	if get.Code != http.StatusOK || !strings.Contains(get.Body.String(), "SUMMARY:DAV habit") || !strings.Contains(get.Body.String(), "X-DAR-HABIT:TRUE") {
		t.Fatalf("GET status=%d body=%s", get.Code, get.Body.String())
	}
	report := httptest.NewRecorder()
	svc.Handler(report, davRequest("REPORT", "/remote.php/dav/calendars/alice/"+list.ID+"/", "", "alice"))
	if report.Code != http.StatusMultiStatus || !strings.Contains(report.Body.String(), "DAV habit") {
		t.Fatalf("REPORT status=%d body=%s", report.Code, report.Body.String())
	}
}
