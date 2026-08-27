package email

import (
	"context"
	"fmt"
	"strings"

	"github.com/anoosa/dar/internal/apps"
	searchindex "github.com/anoosa/dar/internal/search"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

func (a *App) IndexSearch(ctx context.Context, uid string, sink apps.SearchDocumentSink) error {
	return a.withConn(ctx, func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT m.id,m.account_id,m.thread_key,m.subject,m.sender,m.recipients,m.cc,m.bcc,m.text_body,m.snippet,m.sent_at,a.email,
			COALESCE((SELECT group_concat(filename,' ') FROM email_attachments x WHERE x.message_id=m.id),'')
			FROM email_messages m JOIN email_accounts a ON a.id=m.account_id
			WHERE a.user_id=? AND EXISTS(SELECT 1 FROM email_message_locations l WHERE l.message_id=m.id AND l.is_deleted=0)
			ORDER BY m.account_id,m.thread_key,m.sent_at,m.id`,
			&sqlitex.ExecOptions{Args: []any{uid}, ResultFunc: func(stmt *sqlite.Stmt) error {
				messageID := stmt.ColumnInt64(0)
				accountID, key := stmt.ColumnText(1), stmt.ColumnText(2)
				objectID := threadID(accountID, key)
				title := stmt.ColumnText(3)
				if strings.TrimSpace(title) == "" {
					title = "(no subject)"
				}
				participants := strings.Join([]string{stmt.ColumnText(4), stmt.ColumnText(5), stmt.ColumnText(6), stmt.ColumnText(7)}, " ")
				body := stmt.ColumnText(8)
				if body == "" {
					body = stmt.ColumnText(9)
				}
				for i, chunk := range searchindex.ChunkText(body) {
					doc := apps.SearchDocument{
						ObjectID: objectID, ChunkID: fmt.Sprintf("%d-%d", messageID, i), Title: title,
						Subtitle: stmt.ColumnText(4) + " · " + stmt.ColumnText(11),
						Keywords: participants + " " + stmt.ColumnText(12), Body: chunk,
						URL: "/email?thread=" + objectID, Anchor: fmt.Sprintf("message-%d", messageID),
						ModifiedAt: stmt.ColumnInt64(10),
					}
					if err := sink.AddSearchDocument(ctx, doc); err != nil {
						return err
					}
				}
				return nil
			}})
	})
}

func (a *App) markSearchDirty() {
	if a.search != nil {
		a.search.MarkSearchDirty("email")
	}
}
