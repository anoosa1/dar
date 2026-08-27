package email

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"mime/multipart"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/outbound"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/testutil"
)

type emailSearchSink struct {
	docs []apps.SearchDocument
	err  error
}

func (s *emailSearchSink) AddSearchDocument(_ context.Context, doc apps.SearchDocument) error {
	if s.err != nil {
		return s.err
	}
	s.docs = append(s.docs, doc)
	return nil
}

func emailAPIRequest(t *testing.T, app *App, userID, method, target string, body any) *httptest.ResponseRecorder {
	t.Helper()
	var data []byte
	switch value := body.(type) {
	case nil:
	case string:
		data = []byte(value)
	default:
		var err error
		data, err = json.Marshal(value)
		if err != nil {
			t.Fatal(err)
		}
	}
	req := httptest.NewRequest(method, target, bytes.NewReader(data))
	req.Header.Set("Content-Type", "application/json")
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: userID}))
	rec := httptest.NewRecorder()
	app.handleAPI(rec, req)
	return rec
}

func TestEmailAPIAccountFolderDraftMessageSearchLifecycle(t *testing.T) {
	app := testApp(t, testKey(t, "current"))
	accountInput := baseAccount()
	rec := emailAPIRequest(t, app, "alice", http.MethodPost, "/api/email/accounts", accountInput)
	if rec.Code != http.StatusCreated {
		t.Fatalf("create account: %d %s", rec.Code, rec.Body.String())
	}
	var account Account
	if err := json.Unmarshal(rec.Body.Bytes(), &account); err != nil {
		t.Fatal(err)
	}
	if account.ID == "" || account.Password != "" {
		t.Fatalf("account response = %#v", account)
	}

	for _, target := range []string{
		"/api/email",
		"/api/email/accounts",
		"/api/email/accounts/" + account.ID,
		"/api/email/accounts/" + account.ID + "/folders",
		"/api/email/mailboxes?account_id=" + account.ID,
		"/api/email/accounts/" + account.ID + "/sync",
	} {
		rec = emailAPIRequest(t, app, "alice", http.MethodGet, target, nil)
		if rec.Code != http.StatusOK {
			t.Fatalf("GET %s: %d %s", target, rec.Code, rec.Body.String())
		}
	}
	rec = emailAPIRequest(t, app, "bob", http.MethodGet, "/api/email/accounts/"+account.ID, nil)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("cross-user account = %d", rec.Code)
	}

	update := accountInput
	update.Name = "Alice Updated"
	update.Password = ""
	rec = emailAPIRequest(t, app, "alice", http.MethodPatch, "/api/email/accounts/"+account.ID, update)
	if rec.Code != http.StatusOK {
		t.Fatalf("update account: %d %s", rec.Code, rec.Body.String())
	}

	rec = emailAPIRequest(t, app, "alice", http.MethodPost, "/api/email/accounts/"+account.ID+"/folders", map[string]string{"Name": "Projects"})
	if rec.Code != http.StatusCreated {
		t.Fatalf("create folder: %d %s", rec.Code, rec.Body.String())
	}
	var folder Folder
	if err := json.Unmarshal(rec.Body.Bytes(), &folder); err != nil {
		t.Fatal(err)
	}
	rec = emailAPIRequest(t, app, "alice", http.MethodPatch, "/api/email/accounts/"+account.ID+"/folders/"+strconv.FormatInt(folder.ID, 10), map[string]string{"Name": "Roadmap"})
	if rec.Code != http.StatusOK {
		t.Fatalf("rename folder: %d %s", rec.Code, rec.Body.String())
	}

	raw, _, err := buildMessage(outgoingMessage{
		FromName: "Bob", FromEmail: "bob@example.test", To: account.Email,
		Subject: "Operational message", Body: "move and delete workflow", MessageID: "operations@example.test",
	})
	if err != nil {
		t.Fatal(err)
	}
	inbox := filepath.Join(app.mailRoot("alice", account.ID), "INBOX", "new")
	if err = os.MkdirAll(inbox, 0700); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(inbox, "operations:2,"), raw, 0600); err != nil {
		t.Fatal(err)
	}
	if _, err = app.reindexAccount(context.Background(), "alice", account.ID); err != nil {
		t.Fatal(err)
	}
	operationalThreads, _, err := app.threads(context.Background(), "alice", threadQuery{Role: "inbox", Query: "Operational", Limit: 10})
	if err != nil || len(operationalThreads) != 1 {
		t.Fatalf("operational threads = %#v, %v", operationalThreads, err)
	}
	operationalMessages, err := app.threadMessages(context.Background(), "alice", operationalThreads[0].ID)
	if err != nil || len(operationalMessages) != 1 {
		t.Fatalf("operational messages = %#v, %v", operationalMessages, err)
	}
	messageID := operationalMessages[0].ID
	for _, action := range []struct {
		name        string
		destination int64
	}{
		{"archive", 0},
		{"move", folder.ID},
		{"trash", 0},
		{"delete", 0},
	} {
		rec = emailAPIRequest(t, app, "alice", http.MethodPost, "/api/email/messages/actions", map[string]any{
			"action": action.name, "message_ids": []int64{messageID}, "destination_folder": action.destination,
		})
		if rec.Code != http.StatusAccepted {
			t.Fatalf("%s message: %d %s", action.name, rec.Code, rec.Body.String())
		}
	}

	draftInput := DraftInput{
		AccountID: account.ID, To: "Bob <bob@example.test>", Cc: "carol@example.test",
		Subject: "Regression status", Body: "Searchable email draft body",
	}
	rec = emailAPIRequest(t, app, "alice", http.MethodPost, "/api/email/drafts", draftInput)
	if rec.Code != http.StatusCreated {
		t.Fatalf("create draft: %d %s", rec.Code, rec.Body.String())
	}
	var draft Message
	if err := json.Unmarshal(rec.Body.Bytes(), &draft); err != nil {
		t.Fatal(err)
	}
	if draft.ID == 0 || !draft.Draft {
		t.Fatalf("draft = %#v", draft)
	}
	rec = emailAPIRequest(t, app, "alice", http.MethodGet, "/api/email/drafts/"+strconv.FormatInt(draft.ID, 10), nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("get draft: %d %s", rec.Code, rec.Body.String())
	}
	draftInput.Subject = "Updated regression status"
	rec = emailAPIRequest(t, app, "alice", http.MethodPatch, "/api/email/drafts/"+strconv.FormatInt(draft.ID, 10), draftInput)
	if rec.Code != http.StatusOK {
		t.Fatalf("update draft: %d %s", rec.Code, rec.Body.String())
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &draft); err != nil {
		t.Fatal(err)
	}

	for _, target := range []string{
		"/api/email/threads?account_id=" + account.ID + "&role=drafts&q=regression&unread=false&limit=25",
		"/api/email/threads/" + threadID(account.ID, draft.ThreadKey),
		"/api/email/recipients?q=example&limit=10",
	} {
		rec = emailAPIRequest(t, app, "alice", http.MethodGet, target, nil)
		if rec.Code != http.StatusOK {
			t.Fatalf("GET %s: %d %s", target, rec.Code, rec.Body.String())
		}
	}

	for _, action := range []string{"read", "unread", "flag", "unflag"} {
		rec = emailAPIRequest(t, app, "alice", http.MethodPost, "/api/email/messages/actions", map[string]any{
			"action": action, "message_ids": []int64{draft.ID},
		})
		if rec.Code != http.StatusAccepted {
			t.Fatalf("%s draft: %d %s", action, rec.Code, rec.Body.String())
		}
	}

	sink := &emailSearchSink{}
	if err := app.IndexSearch(context.Background(), "alice", sink); err != nil {
		t.Fatal(err)
	}
	if len(sink.docs) == 0 || sink.docs[0].ObjectID != threadID(account.ID, draft.ThreadKey) ||
		!strings.Contains(sink.docs[0].Body, "Searchable") {
		t.Fatalf("search docs = %#v", sink.docs)
	}
	sink.err = context.Canceled
	if err := app.IndexSearch(context.Background(), "alice", sink); !errors.Is(err, context.Canceled) {
		t.Fatalf("search sink error = %v", err)
	}

	prefs := Preferences{DefaultAccount: account.ID, Density: "compact", ReadingPane: "horizontal"}
	rec = emailAPIRequest(t, app, "alice", http.MethodPut, "/api/email/preferences", prefs)
	if rec.Code != http.StatusOK {
		t.Fatalf("save preferences: %d %s", rec.Code, rec.Body.String())
	}
	rec = emailAPIRequest(t, app, "alice", http.MethodGet, "/api/email/preferences", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("get preferences: %d %s", rec.Code, rec.Body.String())
	}

	rec = emailAPIRequest(t, app, "alice", http.MethodDelete, "/api/email/drafts/"+strconv.FormatInt(draft.ID, 10), nil)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("delete draft: %d %s", rec.Code, rec.Body.String())
	}
	rec = emailAPIRequest(t, app, "alice", http.MethodDelete, "/api/email/accounts/"+account.ID+"/folders/"+strconv.FormatInt(folder.ID, 10)+"?delete_messages=true", nil)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("delete folder: %d %s", rec.Code, rec.Body.String())
	}
	rec = emailAPIRequest(t, app, "alice", http.MethodDelete, "/api/email/accounts/"+account.ID, nil)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("delete account: %d %s", rec.Code, rec.Body.String())
	}
}

func TestEmailAPIStrictValidationAndErrorClasses(t *testing.T) {
	app := testApp(t, testKey(t, "current"))
	for _, tc := range []struct {
		method string
		target string
		body   any
		status int
	}{
		{http.MethodPost, "/api/email/accounts", `{"unknown":true}`, 400},
		{http.MethodGet, "/api/email/missing", nil, 404},
		{http.MethodPatch, "/api/email/preferences", nil, 405},
		{http.MethodPost, "/api/email/drafts/nope", nil, 400},
		{http.MethodPost, "/api/email/messages/actions", map[string]any{"action": "unknown", "message_ids": []int64{1}}, 404},
		{http.MethodGet, "/api/email/messages/1/html", nil, 404},
		{http.MethodGet, "/api/email/messages/1/attachments/1", nil, 404},
	} {
		rec := emailAPIRequest(t, app, "alice", tc.method, tc.target, tc.body)
		if rec.Code != tc.status {
			t.Fatalf("%s %s: %d, want %d; body=%s", tc.method, tc.target, rec.Code, tc.status, rec.Body.String())
		}
	}
	if contextBackground() == nil {
		t.Fatal("background context is nil")
	}
	if sqlPlaceholders(3) != "?,?,?" || sqlPlaceholders(0) != "" {
		t.Fatal("SQL placeholders boundary failed")
	}
	if truncateError(strings.Repeat("x", 600)) == "" || len(truncateError("short")) != 5 {
		t.Fatal("error truncation boundary failed")
	}
}

func TestEmailAPIDirectBoundaryFailures(t *testing.T) {
	app := testApp(t, testKey(t, "current"))
	account, err := app.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}

	app.disabledReason = "disabled for test"
	for _, tc := range []struct {
		method string
		path   []string
	}{
		{http.MethodPost, nil},
		{http.MethodPatch, []string{account.ID}},
	} {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(tc.method, "/accounts", strings.NewReader(`{}`))
		app.apiAccounts(rec, req, "alice", tc.path)
		if rec.Code != http.StatusServiceUnavailable {
			t.Fatalf("disabled %s path=%v: %d %s", tc.method, tc.path, rec.Code, rec.Body.String())
		}
	}
	app.disabledReason = ""

	for _, path := range [][]string{{"test"}, {account.ID, "test"}} {
		rec := httptest.NewRecorder()
		app.apiAccounts(rec, httptest.NewRequest(http.MethodPost, "/accounts/test", strings.NewReader(`{`)), "alice", path)
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("malformed account test path=%v: %d %s", path, rec.Code, rec.Body.String())
		}
	}

	rec := httptest.NewRecorder()
	app.apiAccounts(rec, httptest.NewRequest(
		http.MethodPost,
		"/accounts/"+account.ID+"/test",
		strings.NewReader(`{"imap_host":"127.0.0.1","imap_port":1,"imap_security":"none"}`),
	), "alice", []string{account.ID, "test"})
	if rec.Code != http.StatusUnprocessableEntity {
		t.Fatalf("account test with stored password: %d %s", rec.Code, rec.Body.String())
	}

	for _, tc := range []struct {
		method string
		path   []string
		body   string
	}{
		{http.MethodPut, nil, ""},
		{http.MethodPatch, []string{"1"}, `{`},
		{http.MethodPatch, []string{"999999"}, `{"Name":"Missing"}`},
		{http.MethodDelete, []string{"999999"}, ""},
	} {
		rec := httptest.NewRecorder()
		app.apiFolders(rec, httptest.NewRequest(tc.method, "/folders", strings.NewReader(tc.body)), "alice", account.ID, tc.path)
		if rec.Code < http.StatusBadRequest {
			t.Fatalf("folder failure %s path=%v unexpectedly succeeded: %d %s", tc.method, tc.path, rec.Code, rec.Body.String())
		}
	}
}

func TestEmailDraftAttachmentAndHermeticSend(t *testing.T) {
	fixture := testutil.StartSMTP(t, testutil.SMTPOptions{Auth: "PLAIN"})
	host, portText, err := net.SplitHostPort(fixture.Address)
	if err != nil {
		t.Fatal(err)
	}
	port, err := strconv.Atoi(portText)
	if err != nil {
		t.Fatal(err)
	}
	policy, err := outbound.ParsePolicy(host, "")
	if err != nil {
		t.Fatal(err)
	}
	app := testApp(t, testKey(t, "current"))
	app.outbound = policy
	accountInput := baseAccount()
	accountInput.Email = "sender@example.test"
	accountInput.Username = "username"
	accountInput.Password = "password"
	accountInput.SMTPHost = host
	accountInput.SMTPPort = port
	accountInput.SMTPSecurity = securityNone
	rec := emailAPIRequest(t, app, "alice", http.MethodPost, "/api/email/accounts", accountInput)
	if rec.Code != http.StatusCreated {
		t.Fatalf("create account: %d %s", rec.Code, rec.Body.String())
	}
	var account Account
	if err = json.Unmarshal(rec.Body.Bytes(), &account); err != nil {
		t.Fatal(err)
	}
	rec = emailAPIRequest(t, app, "alice", http.MethodPost, "/api/email/drafts", DraftInput{
		AccountID: account.ID, To: "recipient@example.test", Bcc: "hidden@example.test",
		Subject: "Hermetic delivery", Body: "message body",
	})
	if rec.Code != http.StatusCreated {
		t.Fatalf("create draft: %d %s", rec.Code, rec.Body.String())
	}
	var draft Message
	if err = json.Unmarshal(rec.Body.Bytes(), &draft); err != nil {
		t.Fatal(err)
	}

	var body bytes.Buffer
	writer := multipart.NewWriter(&body)
	part, err := writer.CreateFormFile("file", "report.txt")
	if err != nil {
		t.Fatal(err)
	}
	_, _ = part.Write([]byte("attachment data"))
	_ = writer.Close()
	req := httptest.NewRequest(http.MethodPost, "/api/email/drafts/"+strconv.FormatInt(draft.ID, 10)+"/attachments", &body)
	req.Header.Set("Content-Type", writer.FormDataContentType())
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: "alice"}))
	rec = httptest.NewRecorder()
	app.handleAPI(rec, req)
	if rec.Code != http.StatusCreated {
		t.Fatalf("attach file: %d %s", rec.Code, rec.Body.String())
	}
	if err = json.Unmarshal(rec.Body.Bytes(), &draft); err != nil {
		t.Fatal(err)
	}
	if len(draft.Attachments) != 1 {
		t.Fatalf("attachments = %#v", draft.Attachments)
	}
	attachmentTarget := "/api/email/messages/" + strconv.FormatInt(draft.ID, 10) + "/attachments/" + draft.Attachments[0].Part
	rec = emailAPIRequest(t, app, "alice", http.MethodGet, attachmentTarget, nil)
	if rec.Code != http.StatusOK || rec.Body.String() != "attachment data" {
		t.Fatalf("download attachment: %d %q", rec.Code, rec.Body.String())
	}

	rec = emailAPIRequest(t, app, "alice", http.MethodPost, "/api/email/drafts/"+strconv.FormatInt(draft.ID, 10)+"/send", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("send draft: %d %s", rec.Code, rec.Body.String())
	}
	messages := fixture.Messages()
	if len(messages) != 1 || !strings.Contains(string(messages[0].Data), "Hermetic delivery") ||
		strings.Contains(string(messages[0].Data), "hidden@example.test") {
		t.Fatalf("delivered messages = %#v", messages)
	}
	rec = emailAPIRequest(t, app, "alice", http.MethodGet, "/api/email/drafts/"+strconv.FormatInt(draft.ID, 10), nil)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("sent draft remained in drafts: %d %s", rec.Code, rec.Body.String())
	}
}
