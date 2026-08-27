package news

import (
	"context"
	"fmt"
	"html"
	"regexp"
	"strings"

	"github.com/anoosa/dar/internal/apps"
	searchindex "github.com/anoosa/dar/internal/search"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

var searchHTMLTagRE = regexp.MustCompile(`<[^>]*>`)

func (a *App) IndexSearch(ctx context.Context, uid string, sink apps.SearchDocumentSink) error {
	return a.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT i.id,i.title,COALESCE(i.author,''),i.body,i.pub_date,i.last_modified,f.title
			FROM news_items i JOIN news_feeds f ON f.id=i.feed_id WHERE i.user_id=? ORDER BY i.id`,
			&sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(stmt *sqlite.Stmt) error {
				id := stmt.ColumnInt64(0)
				title := stmt.ColumnText(1)
				author := stmt.ColumnText(2)
				body := strings.Join(strings.Fields(html.UnescapeString(searchHTMLTagRE.ReplaceAllString(stmt.ColumnText(3), " "))), " ")
				modified := stmt.ColumnInt64(5)
				if modified == 0 {
					modified = stmt.ColumnInt64(4)
				}
				feed := stmt.ColumnText(6)
				for i, chunk := range searchindex.ChunkText(body) {
					doc := apps.SearchDocument{
						ObjectID: fmt.Sprint(id), ChunkID: fmt.Sprintf("%d", i), Title: nonEmptyNewsTitle(title),
						Subtitle: feed, Keywords: author + " " + feed, Body: chunk,
						URL: "/news", Anchor: fmt.Sprintf("article-%d", id), ModifiedAt: modified,
					}
					if err := sink.AddSearchDocument(ctx, doc); err != nil {
						return err
					}
				}
				return nil
			}})
	})
}

func nonEmptyNewsTitle(value string) string {
	if strings.TrimSpace(value) == "" {
		return "Untitled article"
	}
	return value
}

func (a *App) markSearchDirty() {
	if a.search != nil {
		a.search.MarkSearchDirty("news")
	}
}
