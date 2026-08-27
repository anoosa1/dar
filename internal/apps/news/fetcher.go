package news

import (
	"context"
	"crypto/md5"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"html"
	"io"
	"net/http"
	"net/url"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/anoosa/dar/internal/outbound"
	"github.com/mmcdole/gofeed"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

const (
	feedBodyLimit    = 10 << 20
	articleBodyLimit = 5 << 20
	faviconBodyLimit = 1 << 20
)

var (
	errInvalid  = errors.New("invalid input")
	errNotFound = errors.New("not found")
	errConflict = errors.New("conflict")
	feedLinkRE  = regexp.MustCompile(`(?is)<link\b[^>]*rel=["'][^"']*alternate[^"']*["'][^>]*>`)
	hrefRE      = regexp.MustCompile(`(?i)href\s*=\s*["']([^"']+)["']`)
	typeRE      = regexp.MustCompile(`(?i)type\s*=\s*["']application/(?:rss\+xml|atom\+xml)["']`)
	articleRE   = regexp.MustCompile(`(?is)<article\b[^>]*>(.*?)</article\s*>`)
	mainRE      = regexp.MustCompile(`(?is)<main\b[^>]*>(.*?)</main\s*>`)
	bodyRE      = regexp.MustCompile(`(?is)<body\b[^>]*>(.*?)</body\s*>`)
)

func validateRemoteURL(ctx context.Context, raw string) (*url.URL, error) {
	policy, _ := outbound.ParsePolicy("", "")
	u, err := policy.Validate(ctx, raw)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", errInvalid, err)
	}
	return u, nil
}

func (a *App) httpClient() *http.Client {
	return a.outbound.Client(15 * time.Second)
}

func (a *App) fetchBytes(ctx context.Context, raw string, limit int64, user, pass string) ([]byte, string, http.Header, error) {
	u, err := a.outbound.Validate(ctx, raw)
	if err != nil {
		return nil, "", nil, err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u.String(), nil)
	if err != nil {
		return nil, "", nil, err
	}
	req.Header.Set("User-Agent", "Dar-News/"+compatibilityVersion)
	req.Header.Set("Accept", "application/rss+xml, application/atom+xml, application/xml, text/xml, text/html;q=0.8, */*;q=0.2")
	if user != "" {
		req.SetBasicAuth(user, pass)
	}
	resp, err := a.httpClient().Do(req)
	if err != nil {
		return nil, "", nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, "", nil, fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	reader := io.LimitReader(resp.Body, limit+1)
	data, err := io.ReadAll(reader)
	if err != nil {
		return nil, "", nil, err
	}
	if int64(len(data)) > limit {
		return nil, "", nil, fmt.Errorf("response exceeds %d bytes", limit)
	}
	return data, resp.Request.URL.String(), resp.Header, nil
}

func discoverFeedURL(base string, data []byte) (string, bool) {
	for _, tag := range feedLinkRE.FindAll(data, -1) {
		if !typeRE.Match(tag) {
			continue
		}
		m := hrefRE.FindSubmatch(tag)
		if len(m) != 2 {
			continue
		}
		u, err := url.Parse(html.UnescapeString(string(m[1])))
		if err != nil {
			continue
		}
		b, _ := url.Parse(base)
		return b.ResolveReference(u).String(), true
	}
	return "", false
}

func parseFeed(data []byte, source string) (*gofeed.Feed, error) {
	parser := gofeed.NewParser()
	return parser.Parse(strings.NewReader(string(data)))
}

func hashMD5(s string) string { sum := md5.Sum([]byte(s)); return hex.EncodeToString(sum[:]) }
func hashSHA(s string) string { sum := sha256.Sum256([]byte(s)); return hex.EncodeToString(sum[:]) }

func extractArticleHTML(raw string) string {
	for _, re := range []*regexp.Regexp{articleRE, mainRE, bodyRE} {
		if m := re.FindStringSubmatch(raw); len(m) == 2 {
			return strings.TrimSpace(m[1])
		}
	}
	return ""
}

func (a *App) resolveSubscription(ctx context.Context, raw, user, pass string, auto bool) (*gofeed.Feed, string, error) {
	data, final, _, err := a.fetchBytes(ctx, raw, feedBodyLimit, user, pass)
	if err != nil {
		return nil, "", err
	}
	feed, parseErr := parseFeed(data, final)
	if parseErr == nil && feed.Title != "" {
		return feed, final, nil
	}
	if !auto {
		return nil, "", fmt.Errorf("parse feed: %w", parseErr)
	}
	discovered, ok := discoverFeedURL(final, data)
	if !ok {
		return nil, "", fmt.Errorf("no RSS or Atom feed discovered")
	}
	data, final, _, err = a.fetchBytes(ctx, discovered, feedBodyLimit, user, pass)
	if err != nil {
		return nil, "", err
	}
	feed, err = parseFeed(data, final)
	return feed, final, err
}

func nullable(s string) any {
	if strings.TrimSpace(s) == "" {
		return nil
	}
	return s
}

func itemDate(item *gofeed.Item) int64 {
	if item.PublishedParsed != nil {
		return item.PublishedParsed.Unix()
	}
	if item.UpdatedParsed != nil {
		return item.UpdatedParsed.Unix()
	}
	return time.Now().Unix()
}

func (a *App) updateFeedNow(ctx context.Context, uid string, id int64) error {
	defer a.markSearchDirty()
	f, err := a.feed(ctx, uid, id)
	if err != nil {
		return err
	}
	if !f.UpdateEnabled {
		return nil
	}
	data, final, _, err := a.fetchBytes(ctx, f.URL, feedBodyLimit, f.BasicAuthUser, f.BasicAuthPassword)
	if err != nil {
		a.recordFeedError(ctx, uid, id, err)
		return err
	}
	parsed, err := parseFeed(data, final)
	if err != nil {
		a.recordFeedError(ctx, uid, id, err)
		return err
	}
	now := time.Now().Unix()
	count := 0
	err = a.withConn(ctx, func(c *sqlite.Conn) (err error) {
		end, err := sqlitex.ImmediateTransaction(c)
		if err != nil {
			return err
		}
		defer end(&err)
		for _, entry := range parsed.Items {
			guid := strings.TrimSpace(entry.GUID)
			if guid == "" {
				guid = strings.TrimSpace(entry.Link)
			}
			if guid == "" {
				guid = entry.Title + fmt.Sprint(itemDate(entry))
			}
			guidHash := hashMD5(guid)
			body := entry.Content
			if body == "" {
				body = entry.Description
			}
			body = strings.TrimSpace(body)
			if f.FullTextEnabled && entry.Link != "" {
				if page, _, _, fetchErr := a.fetchBytes(ctx, entry.Link, articleBodyLimit, "", ""); fetchErr == nil {
					if extracted := extractArticleHTML(string(page)); extracted != "" {
						body = extracted
					}
				}
			}
			haystack := strings.ToLower(entry.Title + " " + body + " " + entry.Link)
			hidden := false
			for _, word := range strings.Split(f.KeywordFilters, ",") {
				if word = strings.TrimSpace(strings.ToLower(word)); word != "" && strings.Contains(haystack, word) {
					hidden = true
					break
				}
			}
			if hidden {
				continue
			}
			author := ""
			if entry.Author != nil {
				author = entry.Author.Name
			}
			var enclosureMime, enclosureLink any
			if len(entry.Enclosures) > 0 {
				enclosureMime = nullable(entry.Enclosures[0].Type)
				enclosureLink = nullable(entry.Enclosures[0].URL)
			}
			var mediaThumbnail any
			if entry.Image != nil {
				mediaThumbnail = nullable(entry.Image.URL)
			}
			fingerprint := hashSHA(entry.Title + "\x00" + body + "\x00" + entry.Link)
			existingUnread := 1
			var existingFingerprint string
			_ = sqlitex.Execute(c, `SELECT unread,fingerprint FROM news_items WHERE user_id=? AND feed_id=? AND guid_hash=?`, &sqlitex.ExecOptions{Args: []any{uid, id, guidHash}, ResultFunc: func(s *sqlite.Stmt) error {
				existingUnread = int(s.ColumnInt64(0))
				existingFingerprint = s.ColumnText(1)
				return nil
			}})
			if existingFingerprint != "" && existingFingerprint != fingerprint && f.UpdateMode == 1 {
				existingUnread = 1
			}
			if err := sqlitex.Execute(c, `INSERT INTO news_items(user_id,feed_id,guid,guid_hash,url,title,author,pub_date,body,enclosure_mime,enclosure_link,media_thumbnail,unread,last_modified,fingerprint) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(user_id,feed_id,guid_hash) DO UPDATE SET url=excluded.url,title=excluded.title,author=excluded.author,pub_date=excluded.pub_date,body=excluded.body,enclosure_mime=excluded.enclosure_mime,enclosure_link=excluded.enclosure_link,media_thumbnail=excluded.media_thumbnail,unread=?,last_modified=excluded.last_modified,fingerprint=excluded.fingerprint`, &sqlitex.ExecOptions{Args: []any{uid, id, guid, guidHash, entry.Link, entry.Title, nullable(author), itemDate(entry), body, enclosureMime, enclosureLink, mediaThumbnail, existingUnread, now, fingerprint, existingUnread}}); err != nil {
				return err
			}
			count++
		}
		return sqlitex.Execute(c, `UPDATE news_feeds SET title=?,link=?,favicon_link=?,last_update=?,next_update=?,articles_per_update=?,update_error_count=0,last_update_error='' WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{fallback(parsed.Title, f.Title), parsed.Link, feedFavicon(parsed, f.FaviconLink), now, now + int64(a.refreshMinutes*60), count, uid, id}})
	})
	if err != nil {
		a.recordFeedError(ctx, uid, id, err)
	}
	return err
}

func fallback(v, other string) string {
	if strings.TrimSpace(v) == "" {
		return other
	}
	return v
}

func feedFavicon(feed *gofeed.Feed, current string) string {
	if feed.Image != nil && strings.TrimSpace(feed.Image.URL) != "" {
		return feed.Image.URL
	}
	if current != "" {
		return current
	}
	u, err := url.Parse(feed.Link)
	if err == nil && u.Scheme != "" && u.Host != "" {
		return u.Scheme + "://" + u.Host + "/favicon.ico"
	}
	return ""
}
func (a *App) recordFeedError(ctx context.Context, uid string, id int64, cause error) {
	_ = a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE news_feeds SET update_error_count=update_error_count+1,last_update_error=?,next_update=? WHERE user_id=? AND id=?`, &sqlitex.ExecOptions{Args: []any{cause.Error(), time.Now().Add(time.Duration(a.refreshMinutes) * time.Minute).Unix(), uid, id}})
	})
}

func (a *App) refreshUser(ctx context.Context, uid string) {
	feeds, err := a.feeds(ctx, uid)
	if err != nil {
		return
	}
	var ids []int64
	for _, f := range feeds {
		if f.UpdateEnabled {
			ids = append(ids, f.ID)
		}
	}
	sort.Slice(ids, func(i, j int) bool { return ids[i] < ids[j] })
	a.runJobs(ctx, uid, ids)
	a.recordActivity(ctx, uid, "feeds_refreshed", fmt.Sprintf("refreshed %d news feeds", len(ids)), "sync", uid, "News refresh", "background")
}
func (a *App) runJobs(ctx context.Context, uid string, ids []int64) {
	jobs := make(chan int64)
	workers := 4
	if len(ids) < workers {
		workers = len(ids)
	}
	var wg sync.WaitGroup
	for i := 0; i < workers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for id := range jobs {
				_ = a.updateFeed(ctx, uid, id)
			}
		}()
	}
	for _, id := range ids {
		select {
		case jobs <- id:
		case <-ctx.Done():
			close(jobs)
			wg.Wait()
			return
		}
	}
	close(jobs)
	wg.Wait()
}
