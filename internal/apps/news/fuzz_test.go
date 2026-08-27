package news

import "testing"

func FuzzFeedAndArchiveParsing(f *testing.F) {
	for _, seed := range []string{
		`<?xml version="1.0"?><rss version="2.0"><channel><title>Dar</title><link>https://example.test</link><description>News</description><item><title>One</title><guid>1</guid></item></channel></rss>`,
		`<feed xmlns="http://www.w3.org/2005/Atom"><title>Dar</title><id>dar</id><entry><id>1</id><title>One</title></entry></feed>`,
		`{"version":1,"items":[]}`,
		`<`,
	} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, raw string) {
		if len(raw) > 1<<20 {
			return
		}
		_, _ = parseFeed([]byte(raw), "https://example.test/feed")
		_, _ = decodeArticleArchive([]byte(raw))
		_ = extractArticleHTML(raw)
		_ = canonicalFeedLink(raw)
	})
}
