// Package caldav provides the shared CalDAV HTTP surface used by Calendar and
// Tasks. Component stores keep their domain model authoritative while this
// package owns DAV discovery and collection/object protocol responses.
package caldav

import (
	"fmt"
	"html"
	"io"
	"net/http"
	"path"
	"strings"

	"github.com/anoosa/dar/internal/server"
)

type Collection struct {
	ID         string
	Name       string
	Color      string
	Permission string
}

type Object struct {
	Href string
	Data string
	ETag string
}

// Store is implemented by a component-specific source (VTODO or VEVENT).
// The service merges all registered stores into the user's calendar home.
type Store interface {
	Collections(userID string) ([]Collection, error)
	CreateCollection(userID, name string) (Collection, error)
	Objects(userID, collectionID string) ([]Object, error)
	Object(userID, collectionID, href string) (Object, error)
	PutObject(userID, collectionID, href, data string) (Object, error)
	DeleteObject(userID, collectionID, href string) error
}

type Service struct{ stores []Store }

func New() *Service                     { return &Service{} }
func (s *Service) Register(store Store) { s.stores = append(s.stores, store) }

func (s *Service) Handler(w http.ResponseWriter, r *http.Request) {
	u := server.GetUser(r)
	if u == nil {
		w.Header().Set("WWW-Authenticate", `Basic realm="Dar"`)
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}
	p := strings.Trim(strings.TrimPrefix(strings.TrimPrefix(r.URL.Path, "/remote.php/dav"), "/caldav"), "/")
	if p == "" || p == "calendars" {
		s.home(w, u.ID)
		return
	}
	parts := strings.Split(p, "/")
	if len(parts) > 0 && parts[0] == "calendars" {
		parts = parts[1:]
	}
	if len(parts) > 0 && parts[0] == u.ID {
		parts = parts[1:]
	}
	if len(parts) == 0 {
		s.home(w, u.ID)
		return
	}
	collection := parts[0]
	if len(parts) == 1 {
		s.collection(w, r, u.ID, collection)
		return
	}
	s.object(w, r, u.ID, collection, strings.Join(parts[1:], "/"))
}

func (s *Service) collections(uid string) ([]Collection, error) {
	var out []Collection
	seen := map[string]bool{}
	for _, st := range s.stores {
		v, e := st.Collections(uid)
		if e != nil {
			return nil, e
		}
		for _, c := range v {
			if !seen[c.ID] {
				out = append(out, c)
				seen[c.ID] = true
			}
		}
	}
	return out, nil
}
func (s *Service) store(uid, collection string) Store {
	for _, st := range s.stores {
		cs, e := st.Collections(uid)
		if e != nil {
			continue
		}
		for _, c := range cs {
			if c.ID == collection {
				return st
			}
		}
	}
	return nil
}

func (s *Service) home(w http.ResponseWriter, uid string) {
	cs, err := s.collections(uid)
	if err != nil {
		http.Error(w, "storage failure", 500)
		return
	}
	w.Header().Set("DAV", "1, 2, calendar-access")
	w.Header().Set("Content-Type", "application/xml; charset=utf-8")
	w.WriteHeader(http.StatusMultiStatus)
	_, _ = fmt.Fprintf(w, `<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav"><d:response><d:href>/remote.php/dav/calendars/%s/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>`, html.EscapeString(uid))
	for _, c := range cs {
		_, _ = fmt.Fprintf(w, `<d:response><d:href>/remote.php/dav/calendars/%s/%s/</d:href><d:propstat><d:prop><d:displayname>%s</d:displayname><d:resourcetype><d:collection/><cal:calendar/></d:resourcetype><cal:supported-calendar-component-set><cal:comp name="VEVENT"/><cal:comp name="VTODO"/></cal:supported-calendar-component-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>`, html.EscapeString(uid), html.EscapeString(c.ID), html.EscapeString(c.Name))
	}
	_, _ = w.Write([]byte(`</d:multistatus>`))
}

func (s *Service) collection(w http.ResponseWriter, r *http.Request, uid, id string) {
	st := s.store(uid, id)
	if st == nil {
		if r.Method == "MKCALENDAR" {
			if len(s.stores) == 0 {
				http.Error(w, "not found", 404)
				return
			}
			c, e := s.stores[0].CreateCollection(uid, id)
			if e != nil {
				http.Error(w, e.Error(), 400)
				return
			}
			w.Header().Set("Location", "/remote.php/dav/calendars/"+uid+"/"+c.ID+"/")
			w.WriteHeader(http.StatusCreated)
			return
		}
		http.NotFound(w, r)
		return
	}
	switch r.Method {
	case "OPTIONS":
		w.Header().Set("Allow", "OPTIONS, GET, PUT, DELETE, PROPFIND, REPORT, MKCALENDAR")
		w.Header().Set("DAV", "1, 2, calendar-access")
		w.WriteHeader(http.StatusOK)
	case "PROPFIND":
		s.propfind(w, uid, id, st)
	case "REPORT":
		s.report(w, uid, id, st)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func (s *Service) object(w http.ResponseWriter, r *http.Request, uid, collection, href string) {
	st := s.store(uid, collection)
	if st == nil {
		http.NotFound(w, r)
		return
	}
	if !strings.HasSuffix(href, ".ics") {
		http.Error(w, "expected .ics object", 400)
		return
	}
	switch r.Method {
	case "GET":
		o, e := st.Object(uid, collection, href)
		if e != nil {
			http.NotFound(w, r)
			return
		}
		w.Header().Set("Content-Type", "text/calendar; charset=utf-8")
		w.Header().Set("ETag", o.ETag)
		_, _ = w.Write([]byte(o.Data))
	case "PUT":
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "invalid body", 400)
			return
		}
		o, e := st.PutObject(uid, collection, href, string(body))
		if e != nil {
			http.Error(w, e.Error(), 400)
			return
		}
		w.Header().Set("ETag", o.ETag)
		w.WriteHeader(http.StatusCreated)
	case "DELETE":
		if e := st.DeleteObject(uid, collection, href); e != nil {
			http.NotFound(w, r)
			return
		}
		w.WriteHeader(http.StatusNoContent)
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func (s *Service) propfind(w http.ResponseWriter, uid, id string, st Store) {
	objects, e := st.Objects(uid, id)
	if e != nil {
		http.Error(w, "storage failure", 500)
		return
	}
	w.Header().Set("Content-Type", "application/xml; charset=utf-8")
	w.WriteHeader(http.StatusMultiStatus)
	_, _ = fmt.Fprintf(w, `<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">`)
	for _, o := range objects {
		_, _ = fmt.Fprintf(w, `<d:response><d:href>/remote.php/dav/calendars/%s/%s/%s</d:href><d:propstat><d:prop><d:getetag>%s</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>`, html.EscapeString(uid), html.EscapeString(id), html.EscapeString(path.Base(o.Href)), html.EscapeString(o.ETag))
	}
	_, _ = w.Write([]byte(`</d:multistatus>`))
}
func (s *Service) report(w http.ResponseWriter, uid, id string, st Store) {
	objects, e := st.Objects(uid, id)
	if e != nil {
		http.Error(w, "storage failure", 500)
		return
	}
	w.Header().Set("Content-Type", "application/xml; charset=utf-8")
	w.WriteHeader(http.StatusMultiStatus)
	_, _ = fmt.Fprintf(w, `<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav">`)
	for _, o := range objects {
		_, _ = fmt.Fprintf(w, `<d:response><d:href>/remote.php/dav/calendars/%s/%s/%s</d:href><d:propstat><d:prop><d:getetag>%s</d:getetag><cal:calendar-data>%s</cal:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>`, html.EscapeString(uid), html.EscapeString(id), html.EscapeString(path.Base(o.Href)), html.EscapeString(o.ETag), html.EscapeString(o.Data))
	}
	_, _ = w.Write([]byte(`</d:multistatus>`))
}
