package carddav

import (
	"encoding/xml"
	"io"
	"net/http"
	"strings"
)

const (
	nsDAV     = "DAV:"
	nsCardDAV = "urn:ietf:params:xml:ns:carddav"
	nsCS      = "http://calendarserver.org/ns/"
)

// multistatus response builders use raw XML for reliable namespaces
// (encoding/xml attr placement is awkward for DAV clients).

func writeMultistatus(w http.ResponseWriter, body string) {
	w.Header().Set("Content-Type", "application/xml; charset=utf-8")
	w.WriteHeader(http.StatusMultiStatus)
	io.WriteString(w, `<?xml version="1.0" encoding="utf-8"?>`+"\n")
	io.WriteString(w, body)
}

func msOpen() string {
	return `<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav" xmlns:cs="http://calendarserver.org/ns/">`
}

func msClose() string { return `</d:multistatus>` }

func responseOK(href string, propXML string) string {
	var b strings.Builder
	b.WriteString(`<d:response><d:href>`)
	b.WriteString(xmlEscape(href))
	b.WriteString(`</d:href><d:propstat><d:prop>`)
	b.WriteString(propXML)
	b.WriteString(`</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>`)
	return b.String()
}

func responseNotFound(href string) string {
	var b strings.Builder
	b.WriteString(`<d:response><d:href>`)
	b.WriteString(xmlEscape(href))
	b.WriteString(`</d:href><d:propstat><d:prop/><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>`)
	return b.String()
}

func xmlEscape(s string) string {
	var b strings.Builder
	xml.EscapeText(&b, []byte(s))
	return b.String()
}

// propfindRequest is a minimal parse of PROPFIND body (prop names only).
type propfindRequest struct {
	AllProp bool
	Props   map[string]bool // local name lowercased
}

func parsePropfind(body []byte) propfindRequest {
	req := propfindRequest{Props: map[string]bool{}}
	s := string(body)
	if len(strings.TrimSpace(s)) == 0 || strings.Contains(s, "allprop") {
		req.AllProp = true
		return req
	}
	// Collect local names of requested properties (namespace-agnostic).
	// Match tags like <d:displayname/> or <displayname>.
	lower := strings.ToLower(s)
	for _, name := range []string{
		"resourcetype", "displayname", "getetag", "getctag", "getcontenttype",
		"current-user-principal", "principal-URL", "addressbook-home-set",
		"supported-address-data", "address-data", "owner",
	} {
		if strings.Contains(lower, strings.ToLower(name)) {
			req.Props[strings.ToLower(name)] = true
		}
	}
	if len(req.Props) == 0 {
		req.AllProp = true
	}
	return req
}

func (r propfindRequest) wants(local string) bool {
	if r.AllProp {
		return true
	}
	return r.Props[strings.ToLower(local)]
}

// reportRequest kind.
type reportKind int

const (
	reportUnknown reportKind = iota
	reportMultiget
	reportQuery
)

type reportRequest struct {
	Kind  reportKind
	Hrefs []string // for multiget
}

func parseReport(body []byte) reportRequest {
	s := string(body)
	lower := strings.ToLower(s)
	req := reportRequest{}
	if strings.Contains(lower, "addressbook-multiget") {
		req.Kind = reportMultiget
		decoder := xml.NewDecoder(strings.NewReader(s))
		for {
			token, err := decoder.Token()
			if err != nil {
				break
			}
			start, ok := token.(xml.StartElement)
			if !ok || !strings.EqualFold(start.Name.Local, "href") {
				continue
			}
			var href string
			if decoder.DecodeElement(&href, &start) == nil {
				if href = strings.TrimSpace(href); href != "" {
					req.Hrefs = append(req.Hrefs, href)
				}
			}
		}
		if len(req.Hrefs) == 0 {
			req.Hrefs = extractHrefs(s)
		}
		return req
	}
	if strings.Contains(lower, "addressbook-query") {
		req.Kind = reportQuery
		return req
	}
	req.Kind = reportUnknown
	return req
}

func extractHrefs(s string) []string {
	var out []string
	decoder := xml.NewDecoder(strings.NewReader("<root>" + s + "</root>"))
	decoder.Strict = false
	for {
		token, err := decoder.Token()
		if err != nil {
			break
		}
		start, ok := token.(xml.StartElement)
		if !ok || !strings.EqualFold(start.Name.Local, "href") {
			continue
		}
		var href string
		if decoder.DecodeElement(&href, &start) == nil {
			href = strings.TrimSpace(href)
			if strings.Contains(href, "/") {
				out = append(out, href)
			}
		}
	}
	return out
}
