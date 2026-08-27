package email

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"mime"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/server"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

type apiError struct {
	Code, Error string
	Details     any
}

func writeAPI(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}
func writeErr(w http.ResponseWriter, status int, code string, err error) {
	message := err.Error()
	if status >= 500 {
		log.Printf("email API %s: %v", code, err)
		message = "email service unavailable"
	}
	body := map[string]any{"code": code, "error": message}
	writeAPI(w, status, body)
}
func readAPI(r *http.Request, dst any) error {
	dec := json.NewDecoder(io.LimitReader(r.Body, 2<<20))
	dec.DisallowUnknownFields()
	return dec.Decode(dst)
}
func userID(r *http.Request) string {
	if u := server.GetUser(r); u != nil {
		return u.ID
	}
	return ""
}
func method(w http.ResponseWriter, r *http.Request, want ...string) bool {
	for _, x := range want {
		if r.Method == x {
			return true
		}
	}
	w.Header().Set("Allow", strings.Join(want, ", "))
	writeErr(w, 405, "method_not_allowed", errors.New("method not allowed"))
	return false
}
func splitPath(r *http.Request) []string {
	p := strings.TrimPrefix(r.URL.Path, "/api/email")
	p = strings.Trim(p, "/")
	if p == "" {
		return nil
	}
	return strings.Split(p, "/")
}
func statusForError(err error) (int, string) {
	if errors.Is(err, errNotFound) {
		return 404, "not_found"
	}
	s := strings.ToLower(err.Error())
	switch {
	case strings.Contains(s, "disabled") || strings.Contains(s, "encryption") || strings.Contains(s, "mbsync is not"):
		return 503, "email_disabled"
	case strings.Contains(s, "not supported") || strings.Contains(s, "not empty") || strings.Contains(s, "cannot be"):
		return 409, "conflict"
	case strings.Contains(s, "connection test"):
		return 422, "connection_failed"
	default:
		return 400, "invalid_request"
	}
}
func fail(w http.ResponseWriter, err error) {
	status, code := statusForError(err)
	writeErr(w, status, code, err)
}

func (a *App) handleAPI(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	parts := splitPath(r)
	if len(parts) == 0 {
		writeAPI(w, 200, map[string]any{"status": func() string {
			if a.disabledReason != "" {
				return "disabled"
			}
			return "ok"
		}(), "disabled_reason": func() string {
			if a.disabledReason != "" {
				return "email service unavailable"
			}
			return ""
		}()})
		return
	}
	if r.Method != http.MethodGet && (parts[0] == "accounts" || parts[0] == "messages" || parts[0] == "drafts") {
		if err := a.available(); err != nil {
			fail(w, err)
			return
		}
	}
	switch parts[0] {
	case "discover":
		a.apiDiscover(w, r)
	case "accounts":
		a.apiAccounts(w, r, uid, parts[1:])
	case "mailboxes":
		a.apiMailboxes(w, r, uid)
	case "threads":
		a.apiThreads(w, r, uid, parts[1:])
	case "messages":
		a.apiMessages(w, r, uid, parts[1:])
	case "drafts":
		a.apiDrafts(w, r, uid, parts[1:])
	case "preferences":
		a.apiPreferences(w, r, uid)
	case "recipients":
		a.apiRecipients(w, r, uid)
	default:
		writeErr(w, 404, "not_found", errNotFound)
	}
}

func (a *App) apiDiscover(w http.ResponseWriter, r *http.Request) {
	if !method(w, r, http.MethodPost) {
		return
	}
	var in struct {
		Email string `json:"email"`
	}
	if err := readAPI(r, &in); err != nil {
		fail(w, err)
		return
	}
	result, err := discoverAccount(r.Context(), in.Email, a.outbound)
	if err != nil {
		fail(w, err)
		return
	}
	writeAPI(w, 200, result)
}

func (a *App) apiAccounts(w http.ResponseWriter, r *http.Request, uid string, p []string) {
	if len(p) == 0 {
		switch r.Method {
		case http.MethodGet:
			items, err := a.accounts(r.Context(), uid)
			if err != nil {
				fail(w, err)
				return
			}
			writeAPI(w, 200, map[string]any{"accounts": items})
		case http.MethodPost:
			if err := a.available(); err != nil {
				fail(w, err)
				return
			}
			var in Account
			if err := readAPI(r, &in); err != nil {
				fail(w, err)
				return
			}
			if in.Enabled {
				if result, err := a.testAccount(r.Context(), uid, in); err != nil {
					writeAPI(w, 422, map[string]any{"code": "connection_failed", "error": err.Error(), "details": result})
					return
				}
			}
			out, err := a.createAccount(r.Context(), uid, in)
			if err != nil {
				fail(w, err)
				return
			}
			_ = a.seedSpecialFolders(r.Context(), uid, out.ID)
			if out.Enabled {
				a.queueSync(uid, out.ID, "initial")
			}
			a.recordActivity(r.Context(), uid, "account_created", "added email account "+out.Email, "account", out.ID, out.Email, "api")
			writeAPI(w, 201, out)
		default:
			method(w, r, http.MethodGet, http.MethodPost)
		}
		return
	}
	if p[0] == "test" {
		if !method(w, r, http.MethodPost) {
			return
		}
		var in Account
		if err := readAPI(r, &in); err != nil {
			fail(w, err)
			return
		}
		result, err := a.testAccount(r.Context(), uid, in)
		if err != nil {
			writeAPI(w, 422, map[string]any{"code": "connection_failed", "error": err.Error(), "details": result})
			return
		}
		writeAPI(w, 200, result)
		return
	}
	id := p[0]
	if len(p) == 1 {
		switch r.Method {
		case http.MethodGet:
			out, err := a.account(r.Context(), uid, id)
			if err != nil {
				fail(w, err)
				return
			}
			writeAPI(w, 200, out)
		case http.MethodPatch:
			if err := a.available(); err != nil {
				fail(w, err)
				return
			}
			var in Account
			if err := readAPI(r, &in); err != nil {
				fail(w, err)
				return
			}
			if in.Enabled {
				testInput := in
				testInput.ID = id
				if testInput.Password == "" {
					existing, err := a.account(r.Context(), uid, id)
					if err != nil {
						fail(w, err)
						return
					}
					testInput.Password, err = a.password(r.Context(), uid, existing)
					if err != nil {
						fail(w, err)
						return
					}
				}
				if result, err := a.testAccount(r.Context(), uid, testInput); err != nil {
					writeAPI(w, 422, map[string]any{"code": "connection_failed", "error": err.Error(), "details": result})
					return
				}
			}
			out, err := a.updateAccount(r.Context(), uid, id, in)
			if err != nil {
				fail(w, err)
				return
			}
			a.recordActivity(r.Context(), uid, "account_updated", "updated email account "+out.Email, "account", out.ID, out.Email, "api")
			writeAPI(w, 200, out)
		case http.MethodDelete:
			acct, err := a.account(r.Context(), uid, id)
			if err != nil {
				fail(w, err)
				return
			}
			if err := a.deleteAccount(r.Context(), uid, id); err != nil {
				fail(w, err)
				return
			}
			if err := a.removeManagedMailRoot(uid, acct); err != nil {
				log.Printf("remove Maildir for %s: %v", acct.Email, err)
			}
			a.recordActivity(r.Context(), uid, "account_deleted", "removed email account "+acct.Email, "account", acct.ID, acct.Email, "api")
			w.WriteHeader(204)
		default:
			method(w, r, http.MethodGet, http.MethodPatch, http.MethodDelete)
		}
		return
	}
	switch p[1] {
	case "test":
		if !method(w, r, http.MethodPost) {
			return
		}
		var in Account
		if err := readAPI(r, &in); err != nil {
			fail(w, err)
			return
		}
		existing, existingErr := a.account(r.Context(), uid, id)
		if existingErr != nil {
			fail(w, existingErr)
			return
		}
		if in.IMAPHost == "" {
			var err error
			in = existing
			if in.Password == "" {
				in.Password, err = a.password(r.Context(), uid, existing)
				if err != nil {
					fail(w, err)
					return
				}
			}
		} else if in.Password == "" {
			var err error
			in.Password, err = a.password(r.Context(), uid, existing)
			if err != nil {
				fail(w, err)
				return
			}
		}
		in.ID = id
		result, err := a.testAccount(r.Context(), uid, in)
		if err != nil {
			writeAPI(w, 422, map[string]any{"code": "connection_failed", "error": err.Error(), "details": result})
			return
		}
		writeAPI(w, 200, result)
	case "sync":
		if r.Method == http.MethodPost {
			if _, err := a.account(r.Context(), uid, id); err != nil {
				fail(w, err)
				return
			}
			queued := a.queueSync(uid, id, "manual")
			writeAPI(w, 202, map[string]any{"queued": queued, "status": a.syncStatus(uid, id)})
		} else if r.Method == http.MethodGet {
			if _, err := a.account(r.Context(), uid, id); err != nil {
				fail(w, err)
				return
			}
			writeAPI(w, 200, a.syncStatus(uid, id))
		} else {
			method(w, r, http.MethodGet, http.MethodPost)
		}
	case "folders":
		a.apiFolders(w, r, uid, id, p[2:])
	default:
		writeErr(w, 404, "not_found", errNotFound)
	}
}

func (a *App) seedSpecialFolders(ctx context.Context, uid, accountID string) error {
	for _, x := range []struct{ role, name string }{{"inbox", "INBOX"}, {"sent", "Sent"}, {"drafts", "Drafts"}, {"archive", "Archive"}, {"junk", "Junk"}, {"trash", "Trash"}} {
		dir := folderDiskPath(a.mailRoot(uid, accountID), x.name)
		for _, sub := range []string{"cur", "new", "tmp"} {
			_ = os.MkdirAll(filepath.Join(dir, sub), 0700)
		}
		_ = a.withConn(ctx, func(c *sqlite.Conn) error {
			return sqlitex.Execute(c, `INSERT INTO email_folders(account_id,name,role) VALUES(?,?,?) ON CONFLICT(account_id,name) DO UPDATE SET role=excluded.role`, &sqlitex.ExecOptions{Args: []any{accountID, x.name, x.role}})
		})
	}
	return nil
}

func (a *App) apiFolders(w http.ResponseWriter, r *http.Request, uid, accountID string, p []string) {
	if _, err := a.account(r.Context(), uid, accountID); err != nil {
		fail(w, err)
		return
	}
	if len(p) == 0 {
		if r.Method == http.MethodGet {
			items, err := a.folders(r.Context(), uid, accountID)
			if err != nil {
				fail(w, err)
				return
			}
			writeAPI(w, 200, map[string]any{"folders": items})
			return
		}
		if r.Method == http.MethodPost {
			var in struct{ Name, Role string }
			if err := readAPI(r, &in); err != nil {
				fail(w, err)
				return
			}
			out, err := a.createFolder(r.Context(), uid, accountID, in.Name, in.Role)
			if err != nil {
				fail(w, err)
				return
			}
			a.recordActivity(r.Context(), uid, "folder_created", "created mail folder "+out.Name, "folder", fmt.Sprint(out.ID), out.Name, "api")
			writeAPI(w, 201, out)
			return
		}
		method(w, r, http.MethodGet, http.MethodPost)
		return
	}
	id, err := strconv.ParseInt(p[0], 10, 64)
	if err != nil {
		fail(w, err)
		return
	}
	if r.Method == http.MethodPatch {
		var in struct{ Name, Role string }
		if err := readAPI(r, &in); err != nil {
			fail(w, err)
			return
		}
		out, err := a.renameFolder(r.Context(), uid, id, in.Name, in.Role)
		if err != nil {
			fail(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "folder_updated", "updated mail folder "+out.Name, "folder", fmt.Sprint(out.ID), out.Name, "api")
		writeAPI(w, 200, out)
		return
	}
	if r.Method == http.MethodDelete {
		confirm := r.URL.Query().Get("delete_messages") == "true"
		if err := a.deleteFolder(r.Context(), uid, id, confirm); err != nil {
			fail(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "folder_deleted", "deleted mail folder", "folder", fmt.Sprint(id), fmt.Sprint(id), "api")
		w.WriteHeader(204)
		return
	}
	method(w, r, http.MethodPatch, http.MethodDelete)
}

func (a *App) apiMailboxes(w http.ResponseWriter, r *http.Request, uid string) {
	if !method(w, r, http.MethodGet) {
		return
	}
	items, err := a.folders(r.Context(), uid, r.URL.Query().Get("account_id"))
	if err != nil {
		fail(w, err)
		return
	}
	virtual := []map[string]any{}
	for _, role := range []string{"inbox", "starred", "drafts", "sent", "archive", "junk", "trash"} {
		total, unread := 0, 0
		if role == "starred" {
			err = a.withConn(r.Context(), func(c *sqlite.Conn) error {
				return sqlitex.Execute(c, `SELECT count(DISTINCT m.id),count(DISTINCT CASE WHEN l.is_read=0 THEN m.id END) FROM email_messages m JOIN email_accounts a ON a.id=m.account_id JOIN email_message_locations l ON l.message_id=m.id WHERE a.user_id=? AND (?='' OR a.id=?) AND l.is_flagged=1`, &sqlitex.ExecOptions{Args: []any{uid, r.URL.Query().Get("account_id"), r.URL.Query().Get("account_id")}, ResultFunc: func(s *sqlite.Stmt) error {
					total, unread = int(s.ColumnInt64(0)), int(s.ColumnInt64(1))
					return nil
				}})
			})
			if err != nil {
				fail(w, err)
				return
			}
		} else {
			for _, f := range items {
				if f.Role == role {
					total += f.Total
					unread += f.Unread
				}
			}
		}
		virtual = append(virtual, map[string]any{"role": role, "total": total, "unread": unread})
	}
	writeAPI(w, 200, map[string]any{"virtual": virtual, "folders": items})
}

func (a *App) apiThreads(w http.ResponseWriter, r *http.Request, uid string, p []string) {
	if !method(w, r, http.MethodGet) {
		return
	}
	if len(p) > 0 {
		items, err := a.threadMessages(r.Context(), uid, p[0])
		if err != nil {
			fail(w, err)
			return
		}
		writeAPI(w, 200, map[string]any{"messages": items})
		return
	}
	folderID, err := queryInt(r.URL.Query().Get("folder_id"))
	if err != nil {
		fail(w, err)
		return
	}
	before, _ := queryInt(r.URL.Query().Get("before"))
	after, _ := queryInt(r.URL.Query().Get("after"))
	limit, _ := strconv.Atoi(r.URL.Query().Get("limit"))
	items, next, err := a.threads(r.Context(), uid, threadQuery{AccountID: r.URL.Query().Get("account_id"), Role: r.URL.Query().Get("role"), FolderID: folderID, Query: r.URL.Query().Get("q"), Unread: r.URL.Query().Get("unread") == "true", Flagged: r.URL.Query().Get("flagged") == "true", HasAttachment: r.URL.Query().Get("has_attachment") == "true", Before: before, After: after, Cursor: r.URL.Query().Get("cursor"), Limit: limit})
	if err != nil {
		fail(w, err)
		return
	}
	writeAPI(w, 200, map[string]any{"threads": items, "next_cursor": next})
}

func (a *App) apiMessages(w http.ResponseWriter, r *http.Request, uid string, p []string) {
	if len(p) == 1 && p[0] == "actions" {
		if !method(w, r, http.MethodPost) {
			return
		}
		var in struct {
			Action            string  `json:"action"`
			MessageIDs        []int64 `json:"message_ids"`
			DestinationFolder int64   `json:"destination_folder"`
		}
		if err := readAPI(r, &in); err != nil {
			fail(w, err)
			return
		}
		if err := a.applyMessageAction(r.Context(), uid, in.Action, in.MessageIDs, in.DestinationFolder); err != nil {
			fail(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "messages_"+in.Action, fmt.Sprintf("%s %d messages", in.Action, len(in.MessageIDs)), "messages", uid, "Messages", "api")
		writeAPI(w, 202, map[string]string{"status": "queued"})
		return
	}
	if len(p) < 2 {
		writeErr(w, 404, "not_found", errNotFound)
		return
	}
	id, err := strconv.ParseInt(p[0], 10, 64)
	if err != nil {
		fail(w, err)
		return
	}
	switch p[1] {
	case "html":
		a.serveMessageHTML(w, r, uid, id)
	case "attachments":
		if len(p) < 3 {
			writeErr(w, 404, "not_found", errNotFound)
			return
		}
		a.serveAttachment(w, r, uid, id, p[2])
	default:
		writeErr(w, 404, "not_found", errNotFound)
	}
}

func (a *App) serveMessageHTML(w http.ResponseWriter, r *http.Request, uid string, id int64) {
	if !method(w, r, http.MethodGet) {
		return
	}
	var html string
	var attachments []Attachment
	err := a.withConn(r.Context(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT m.html_body FROM email_messages m JOIN email_accounts a ON a.id=m.account_id WHERE a.user_id=? AND m.id=?`, &sqlitex.ExecOptions{Args: []any{uid, id}, ResultFunc: func(s *sqlite.Stmt) error { html = s.ColumnText(0); return nil }})
	})
	if err != nil || html == "" {
		if err == nil {
			err = errNotFound
		}
		fail(w, err)
		return
	}
	attachments, _ = a.attachments(r.Context(), uid, id)
	for _, att := range attachments {
		if att.ContentID != "" {
			html = strings.ReplaceAll(html, "cid:"+att.ContentID, "/api/email/messages/"+fmt.Sprint(id)+"/attachments/"+att.Part)
		}
	}
	imgSrc := "'self' data:"
	if r.URL.Query().Get("remote_images") == "true" {
		imgSrc += " https: http:"
	}
	w.Header().Set("Content-Security-Policy", "sandbox; default-src 'none'; img-src "+imgSrc+"; style-src 'unsafe-inline'; form-action 'none'; frame-src 'none'; frame-ancestors 'self'; connect-src 'none'; base-uri 'none'")
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.Header().Set("X-Frame-Options", "SAMEORIGIN")
	_, _ = io.WriteString(w, html)
}

func (a *App) serveAttachment(w http.ResponseWriter, r *http.Request, uid string, id int64, part string) {
	if !method(w, r, http.MethodGet) {
		return
	}
	path, _, err := a.messagePath(r.Context(), uid, id)
	if err != nil {
		fail(w, err)
		return
	}
	att, err := extractPart(path, part)
	if err != nil {
		fail(w, err)
		return
	}
	name := filepath.Base(att.Filename)
	w.Header().Set("Content-Type", att.MIMEType)
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.Header().Set("Content-Disposition", mime.FormatMediaType("attachment", map[string]string{"filename": name}))
	w.Header().Set("Content-Length", fmt.Sprint(len(att.Data)))
	_, _ = w.Write(att.Data)
}

func draftInput(m Message) DraftInput {
	return DraftInput{AccountID: m.AccountID, To: m.To, Cc: m.Cc, Bcc: m.Bcc, Subject: m.Subject, Body: m.Text}
}
func (a *App) apiDrafts(w http.ResponseWriter, r *http.Request, uid string, p []string) {
	if len(p) == 0 {
		if !method(w, r, http.MethodPost) {
			return
		}
		var in DraftInput
		if err := readAPI(r, &in); err != nil {
			fail(w, err)
			return
		}
		out, err := a.saveDraft(r.Context(), uid, 0, in, nil, "")
		if err != nil {
			fail(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "draft_created", "created email draft "+out.Subject, "message", fmt.Sprint(out.ID), out.Subject, "api")
		writeAPI(w, 201, out)
		return
	}
	id, err := strconv.ParseInt(p[0], 10, 64)
	if err != nil {
		fail(w, err)
		return
	}
	if len(p) == 1 {
		switch r.Method {
		case http.MethodGet:
			out, err := a.withMessage(r.Context(), uid, id)
			if err != nil {
				fail(w, err)
				return
			}
			if !out.Draft {
				fail(w, errors.New("message is not a draft"))
				return
			}
			writeAPI(w, 200, out)
		case http.MethodPatch:
			var in DraftInput
			if err := readAPI(r, &in); err != nil {
				fail(w, err)
				return
			}
			out, err := a.saveDraft(r.Context(), uid, id, in, nil, "")
			if err != nil {
				fail(w, err)
				return
			}
			a.recordActivity(r.Context(), uid, "draft_updated", "updated email draft "+out.Subject, "message", fmt.Sprint(out.ID), out.Subject, "api")
			writeAPI(w, 200, out)
		case http.MethodDelete:
			if err := a.deleteDraft(r.Context(), uid, id); err != nil {
				fail(w, err)
				return
			}
			a.recordActivity(r.Context(), uid, "draft_deleted", "deleted an email draft", "message", fmt.Sprint(id), fmt.Sprint(id), "api")
			w.WriteHeader(204)
		default:
			method(w, r, http.MethodGet, http.MethodPatch, http.MethodDelete)
		}
		return
	}
	switch p[1] {
	case "send":
		if !method(w, r, http.MethodPost) {
			return
		}
		if err := a.sendDraft(r.Context(), uid, id); err != nil {
			writeErr(w, 422, "send_failed", err)
			return
		}
		a.recordActivity(r.Context(), uid, "message_sent", "sent an email", "message", fmt.Sprint(id), fmt.Sprint(id), "api")
		writeAPI(w, 200, map[string]string{"status": "sent"})
	case "attachments":
		a.apiDraftAttachment(w, r, uid, id, p[2:])
	default:
		writeErr(w, 404, "not_found", errNotFound)
	}
}

func (a *App) apiDraftAttachment(w http.ResponseWriter, r *http.Request, uid string, id int64, p []string) {
	existing, err := a.withMessage(r.Context(), uid, id)
	if err != nil {
		fail(w, err)
		return
	}
	in := draftInput(existing)
	if r.Method == http.MethodPost && len(p) == 0 {
		r.Body = http.MaxBytesReader(w, r.Body, 26<<20)
		if err := r.ParseMultipartForm(26 << 20); err != nil {
			fail(w, err)
			return
		}
		file, header, err := r.FormFile("file")
		if err != nil {
			fail(w, err)
			return
		}
		defer file.Close()
		data, err := io.ReadAll(io.LimitReader(file, 25<<20))
		if err != nil {
			fail(w, err)
			return
		}
		if len(data) >= 25<<20 {
			fail(w, errors.New("attachment exceeds 25 MiB"))
			return
		}
		typ := header.Header.Get("Content-Type")
		out, err := a.saveDraft(r.Context(), uid, id, in, &outgoingAttachment{Filename: header.Filename, MIMEType: typ, Data: data}, "")
		if err != nil {
			fail(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "attachment_added", "added an attachment to a draft", "message", fmt.Sprint(id), out.Subject, "api")
		writeAPI(w, 201, out)
		return
	}
	if r.Method == http.MethodDelete && len(p) == 1 {
		out, err := a.saveDraft(r.Context(), uid, id, in, nil, p[0])
		if err != nil {
			fail(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "attachment_removed", "removed an attachment from a draft", "message", fmt.Sprint(id), out.Subject, "api")
		writeAPI(w, 200, out)
		return
	}
	method(w, r, http.MethodPost, http.MethodDelete)
}

func (a *App) apiPreferences(w http.ResponseWriter, r *http.Request, uid string) {
	if r.Method == http.MethodGet {
		out, err := a.preferences(r.Context(), uid)
		if err != nil {
			fail(w, err)
			return
		}
		writeAPI(w, 200, out)
		return
	}
	if r.Method == http.MethodPut {
		var in Preferences
		if err := readAPI(r, &in); err != nil {
			fail(w, err)
			return
		}
		if in.DefaultAccount != "" {
			if _, err := a.account(r.Context(), uid, in.DefaultAccount); err != nil {
				fail(w, err)
				return
			}
		}
		if err := a.savePreferences(r.Context(), uid, in); err != nil {
			fail(w, err)
			return
		}
		a.recordActivity(r.Context(), uid, "preferences_updated", "updated Email preferences", "preferences", uid, "Email preferences", "api")
		writeAPI(w, 200, in)
		return
	}
	method(w, r, http.MethodGet, http.MethodPut)
}

func (a *App) apiRecipients(w http.ResponseWriter, r *http.Request, uid string) {
	if !method(w, r, http.MethodGet) {
		return
	}
	q := r.URL.Query().Get("q")
	limit, _ := strconv.Atoi(r.URL.Query().Get("limit"))
	if limit <= 0 || limit > 30 {
		limit = 12
	}
	seen := map[string]bool{}
	var out []apps.RecipientSuggestion
	if a.recipients != nil {
		items, _ := a.recipients.SuggestRecipients(r.Context(), uid, q, limit)
		for _, x := range items {
			key := strings.ToLower(x.Email)
			if key != "" && !seen[key] {
				seen[key] = true
				out = append(out, x)
			}
		}
	}
	recent, _ := a.recentRecipients(r.Context(), uid, q, limit)
	for _, x := range recent {
		key := strings.ToLower(x.Email)
		if key != "" && !seen[key] {
			seen[key] = true
			out = append(out, apps.RecipientSuggestion{Name: x.Name, Email: x.Email})
		}
	}
	if len(out) > limit {
		out = out[:limit]
	}
	writeAPI(w, 200, map[string]any{"recipients": out})
}

func contextBackground() context.Context { return context.Background() }
