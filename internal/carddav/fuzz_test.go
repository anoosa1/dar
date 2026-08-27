package carddav

import "testing"

func FuzzDAVXMLBodies(f *testing.F) {
	for _, seed := range []string{
		`<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:getetag/></d:prop></d:propfind>`,
		`<card:addressbook-query xmlns:card="urn:ietf:params:xml:ns:carddav"><d:prop xmlns:d="DAV:"><card:address-data/></d:prop></card:addressbook-query>`,
		`<d:sync-collection xmlns:d="DAV:"><d:sync-token>token</d:sync-token></d:sync-collection>`,
		`<`,
	} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, body string) {
		if len(body) > 1<<20 {
			return
		}
		propfind := parsePropfind([]byte(body))
		_ = propfind.wants("getetag")
		report := parseReport([]byte(body))
		_ = report
		_ = extractHrefs(body)
	})
}
