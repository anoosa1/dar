package carddav

import (
	"strings"
	"testing"
)

func TestResponseNotFoundEscapesHref(t *testing.T) {
	got := responseNotFound(`/addressbooks/alice/a&b<card>`)
	if !strings.Contains(got, "404 Not Found") || strings.Contains(got, "a&b<card>") ||
		!strings.Contains(got, "a&amp;b&lt;card&gt;") {
		t.Fatalf("not-found DAV response=%q", got)
	}
}

func TestReportParsingAndHrefFallbackEdges(t *testing.T) {
	multiget := parseReport([]byte(`<card:addressbook-multiget xmlns:card="urn:ietf:params:xml:ns:carddav"><d:href xmlns:d="DAV:">/dav/addressbooks/u/default/a.vcf</d:href><D:HREF xmlns:D="DAV:">relative/b.vcf</D:HREF></card:addressbook-multiget>`))
	if multiget.Kind != reportMultiget || len(multiget.Hrefs) != 2 {
		t.Fatalf("multiget=%+v", multiget)
	}
	if query := parseReport([]byte(`<card:addressbook-query xmlns:card="urn:ietf:params:xml:ns:carddav"/>`)); query.Kind != reportQuery {
		t.Fatalf("query=%+v", query)
	}
	if unknown := parseReport([]byte(`<unknown/>`)); unknown.Kind != reportUnknown {
		t.Fatalf("unknown=%+v", unknown)
	}
	for _, tc := range []struct {
		body string
		want int
	}{
		{`<d:href>/dav/a.vcf</d:href>`, 1},
		{`<href>relative/a.vcf</href>`, 1},
		{`<href>no-slash</href>`, 0},
		{`<href></href>`, 0},
		{`<href`, 0},
		{`nothing`, 0},
	} {
		if got := extractHrefs(tc.body); len(got) != tc.want {
			t.Errorf("extractHrefs(%q)=%v want len=%d", tc.body, got, tc.want)
		}
	}
}
