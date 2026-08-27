package email

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

func TestMailWidgetSettingsSelectAccountAndIncludeReadThreads(t *testing.T) {
	app := testApp(t, testKey(t, "current"))
	account, err := app.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}
	var folderID, messageID int64
	err = app.withConn(context.Background(), func(conn *sqlite.Conn) error {
		if err := sqlitex.Execute(conn, `INSERT INTO email_folders(account_id,name,role) VALUES(?,'INBOX','inbox') RETURNING id`, &sqlitex.ExecOptions{Args: []any{account.ID}, ResultFunc: func(stmt *sqlite.Stmt) error {
			folderID = stmt.ColumnInt64(0)
			return nil
		}}); err != nil {
			return err
		}
		if err := sqlitex.Execute(conn, `INSERT INTO email_messages(account_id,content_hash,thread_key,sender,subject,sent_at,snippet) VALUES(?,?,?,?,?,?,?) RETURNING id`, &sqlitex.ExecOptions{Args: []any{account.ID, "hash", "thread", "Sender", "Already read", time.Now().Unix(), "Preview"}, ResultFunc: func(stmt *sqlite.Stmt) error {
			messageID = stmt.ColumnInt64(0)
			return nil
		}}); err != nil {
			return err
		}
		return sqlitex.Execute(conn, `INSERT INTO email_message_locations(message_id,folder_id,maildir_path,file_size,file_mtime,is_read) VALUES(?,?,?,?,?,1)`, &sqlitex.ExecOptions{Args: []any{messageID, folderID, "cur/message", 10, time.Now().Unix()}})
	})
	if err != nil {
		t.Fatal(err)
	}
	request := apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: apps.WidgetStandard}
	if _, err := app.SaveDashboardWidgetSettings(context.Background(), request, "email.unread", map[string]string{"account_id": account.ID}); err != nil {
		t.Fatal(err)
	}
	data, err := app.DashboardWidget(context.Background(), request, "email.unread")
	if err != nil {
		t.Fatal(err)
	}
	if len(data.Items) != 1 || data.Items[0].Title != "Already read" {
		t.Fatalf("items = %#v", data.Items)
	}
	if data.Items[0].URL == "/email" {
		t.Fatalf("thread deep link = %q", data.Items[0].URL)
	}
}

func TestMailWidgetUsesSemanticSizeLimits(t *testing.T) {
	app := testApp(t, testKey(t, "current"))
	account, err := app.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}
	err = app.withConn(context.Background(), func(conn *sqlite.Conn) error {
		var folderID int64
		if err := sqlitex.Execute(conn, `INSERT INTO email_folders(account_id,name,role) VALUES(?,'INBOX','inbox') RETURNING id`, &sqlitex.ExecOptions{Args: []any{account.ID}, ResultFunc: func(stmt *sqlite.Stmt) error {
			folderID = stmt.ColumnInt64(0)
			return nil
		}}); err != nil {
			return err
		}
		for i := range 12 {
			var messageID int64
			if err := sqlitex.Execute(conn, `INSERT INTO email_messages(account_id,content_hash,thread_key,sender,subject,sent_at,snippet) VALUES(?,?,?,?,?,?,?) RETURNING id`, &sqlitex.ExecOptions{
				Args: []any{account.ID, fmt.Sprintf("hash-%02d", i), fmt.Sprintf("thread-%02d", i), "Sender", fmt.Sprintf("Message %02d", i), time.Now().Add(time.Duration(i) * time.Minute).Unix(), "Preview"},
				ResultFunc: func(stmt *sqlite.Stmt) error {
					messageID = stmt.ColumnInt64(0)
					return nil
				},
			}); err != nil {
				return err
			}
			if err := sqlitex.Execute(conn, `INSERT INTO email_message_locations(message_id,folder_id,maildir_path,file_size,file_mtime,is_read) VALUES(?,?,?,?,?,?)`, &sqlitex.ExecOptions{
				Args: []any{messageID, folderID, fmt.Sprintf("cur/message-%02d", i), 10, time.Now().Unix(), 0},
			}); err != nil {
				return err
			}
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	for size, want := range map[apps.WidgetSize]int{
		apps.WidgetCompact:  1,
		apps.WidgetStandard: 3,
		apps.WidgetWide:     6,
		apps.WidgetLarge:    10,
	} {
		data, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: size}, "email.unread")
		if err != nil {
			t.Fatal(err)
		}
		if len(data.Items) != want {
			t.Fatalf("%s items = %d, want %d", size, len(data.Items), want)
		}
	}
}
