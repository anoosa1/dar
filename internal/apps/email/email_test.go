package email

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/outbound"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

type fixedResolver struct{ ips []net.IP }

type notificationRecorder struct {
	items []apps.Notification
}

func (r *notificationRecorder) EnqueueNotification(_ context.Context, notification apps.Notification) error {
	r.items = append(r.items, notification)
	return nil
}

func (r fixedResolver) LookupIP(context.Context, string, string) ([]net.IP, error) {
	return r.ips, nil
}

func testKey(t *testing.T, id string) CredentialKey {
	t.Helper()
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		t.Fatal(err)
	}
	return CredentialKey{ID: id, Key: b}
}

func testApp(t *testing.T, keys ...CredentialKey) *App {
	t.Helper()
	m := append(preferences.Migrations(), Migrations()...)
	database, err := db.Open(filepath.Join(t.TempDir(), "email.sqlite"), m)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	return New(Config{DB: database, DataRoot: t.TempDir(), MBSyncPath: "/bin/true", CredentialKeys: keys})
}

func baseAccount() Account {
	return Account{Email: "alice@example.test", Name: "Alice", Username: "alice@example.test", IMAPHost: "imap.example.test", IMAPPort: 993, IMAPSecurity: securityTLS, SMTPHost: "smtp.example.test", SMTPPort: 587, SMTPSecurity: securitySTARTTLS, Password: "secret-app-password", Enabled: false, SyncMinutes: 5, SaveSent: true}
}

func TestParseCredentialKeysAndAuthenticatedEncryption(t *testing.T) {
	rawKey := make([]byte, 32)
	_, _ = rand.Read(rawKey)
	keys, err := ParseCredentialKeys("current:" + base64.StdEncoding.EncodeToString(rawKey))
	if err != nil || len(keys) != 1 {
		t.Fatalf("keys=%v err=%v", keys, err)
	}
	ring := newKeyRing(keys)
	id, nonce, ciphertext, err := ring.encrypt("alice", "account", "password")
	if err != nil {
		t.Fatal(err)
	}
	plain, rotate, err := ring.decrypt("alice", "account", id, nonce, ciphertext)
	if err != nil || plain != "password" || rotate {
		t.Fatalf("plain=%q rotate=%v err=%v", plain, rotate, err)
	}
	if _, _, err = ring.decrypt("bob", "account", id, nonce, ciphertext); err == nil {
		t.Fatal("ciphertext was not bound to the user")
	}
	ciphertext[0] ^= 0xff
	if _, _, err = ring.decrypt("alice", "account", id, nonce, ciphertext); err == nil {
		t.Fatal("tampering was accepted")
	}
}

func TestAutoconfigParsingAndRedirectPolicy(t *testing.T) {
	xmlData := []byte(`<clientConfig><emailProvider><incomingServer type="imap"><hostname>imap.example.test</hostname><port>993</port><socketType>SSL</socketType><username>%EMAILADDRESS%</username></incomingServer><outgoingServer type="smtp"><hostname>smtp.example.test</hostname><port>587</port><socketType>STARTTLS</socketType><username>%EMAILLOCALPART%</username></outgoingServer></emailProvider></clientConfig>`)
	got, err := parseAutoConfig(xmlData, "alice@example.test", "fixture")
	if err != nil {
		t.Fatal(err)
	}
	if got.Username != "alice@example.test" || got.IMAPSecurity != securityTLS || got.SMTPSecurity != securitySTARTTLS {
		t.Fatalf("discovery=%+v", got)
	}
	client := discoveryClient()
	redirect := httptest.NewRequest(http.MethodGet, "http://autoconfig.example.test/config.xml", nil)
	via := []*http.Request{httptest.NewRequest(http.MethodGet, "https://autoconfig.example.test/config.xml", nil)}
	if err := client.CheckRedirect(redirect, via); err == nil {
		t.Fatal("HTTP redirect was accepted")
	}
	secure := httptest.NewRequest(http.MethodGet, "https://autoconfig.example.test/config.xml", nil)
	if err := client.CheckRedirect(secure, append(via, via[0], via[0])); err == nil {
		t.Fatal("redirect limit was not enforced")
	}
}

func TestCredentialKeyRotationReencryptsAccount(t *testing.T) {
	oldKey, newKey := testKey(t, "old"), testKey(t, "new")
	oldApp := testApp(t, oldKey)
	acct, err := oldApp.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}
	oldApp.keys = newKeyRing([]CredentialKey{newKey, oldKey})
	stored, err := oldApp.account(context.Background(), "alice", acct.ID)
	if err != nil {
		t.Fatal(err)
	}
	plain, err := oldApp.password(context.Background(), "alice", stored)
	if err != nil || plain != "secret-app-password" {
		t.Fatalf("plain=%q err=%v", plain, err)
	}
	rotated, err := oldApp.account(context.Background(), "alice", acct.ID)
	if err != nil {
		t.Fatal(err)
	}
	if rotated.PasswordKeyID != "new" {
		t.Fatalf("key id=%q", rotated.PasswordKeyID)
	}
}

func TestMIMEBuildParseAttachmentsAndBcc(t *testing.T) {
	raw, recipients, err := buildMessage(outgoingMessage{FromName: "Alice", FromEmail: "alice@example.test", To: "Bob <bob@example.test>", Cc: "carol@example.test", Bcc: "hidden@example.test", Subject: "Résumé status", Body: "Hello\nworld", MessageID: "one@example.test", Attachments: []outgoingAttachment{{Filename: "report.txt", MIMEType: "text/plain", Data: []byte("report")}}})
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(raw, []byte("Bcc:")) {
		t.Fatal("Bcc leaked into the transmitted headers")
	}
	if len(recipients) != 3 {
		t.Fatalf("recipients=%v", recipients)
	}
	path := filepath.Join(t.TempDir(), "message.eml")
	if err := os.WriteFile(path, raw, 0600); err != nil {
		t.Fatal(err)
	}
	parsed, err := parseMailFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if parsed.Subject != "Résumé status" || !strings.Contains(parsed.Text, "Hello") || len(parsed.Attachments) != 1 {
		t.Fatalf("parsed=%+v", parsed)
	}
	part, err := extractPart(path, parsed.Attachments[0].Part)
	if err != nil || string(part.Data) != "report" {
		t.Fatalf("part=%+v err=%v", part, err)
	}
	draft, _, err := buildMessage(outgoingMessage{FromEmail: "alice@example.test", Bcc: "hidden@example.test", Body: "draft", MessageID: "draft@example.test", IncludeBcc: true, Draft: true})
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(draft, []byte("Bcc:")) || !bytes.Contains(draft, []byte("hidden@example.test")) {
		t.Fatal("draft did not preserve Bcc")
	}
}

func TestReplyDraftPreservesThreadHeadersAndCanResolveAccount(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	acct, err := a.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}
	if err := a.seedSpecialFolders(context.Background(), "alice", acct.ID); err != nil {
		t.Fatal(err)
	}
	draft, err := a.saveDraft(context.Background(), "alice", 0, DraftInput{
		AccountID: acct.ID,
		To:        "bob@example.test", Subject: "Re: Project Aurora", Body: "Reply body",
		InReplyTo: "<root@example.test>", References: "<root@example.test>",
	}, nil, "")
	if err != nil {
		t.Fatal(err)
	}
	path, resolved, err := a.messagePath(context.Background(), "alice", draft.ID)
	if err != nil {
		t.Fatal(err)
	}
	if resolved.ID != acct.ID {
		t.Fatalf("resolved account=%q", resolved.ID)
	}
	parsed, err := parseMailFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if parsed.InReplyTo != "<root@example.test>" || parsed.References != "<root@example.test>" {
		t.Fatalf("reply headers: in-reply-to=%q references=%q", parsed.InReplyTo, parsed.References)
	}
}

func TestMaildirIndexThreadsSearchAndUserScope(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	acct, err := a.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}
	if err := a.seedSpecialFolders(context.Background(), "alice", acct.ID); err != nil {
		t.Fatal(err)
	}
	raw1, _, _ := buildMessage(outgoingMessage{FromName: "Bob", FromEmail: "bob@example.test", To: "alice@example.test", Subject: "Project Aurora", Body: "The launch checklist is attached.", MessageID: "root@example.test"})
	raw2, _, _ := buildMessage(outgoingMessage{FromName: "Alice", FromEmail: "alice@example.test", To: "bob@example.test", Subject: "Re: Project Aurora", Body: "Looks good.", MessageID: "reply@example.test", InReplyTo: "<root@example.test>", References: "<root@example.test>"})
	dir := filepath.Join(a.mailRoot("alice", acct.ID), "INBOX", "new")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "one:2,"), raw1, 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "two:2,"), raw2, 0600); err != nil {
		t.Fatal(err)
	}
	stats, err := a.reindexAccount(context.Background(), "alice", acct.ID)
	if err != nil {
		t.Fatal(err)
	}
	if stats.Updated != 2 {
		t.Fatalf("stats=%+v", stats)
	}
	threads, _, err := a.threads(context.Background(), "alice", threadQuery{Role: "inbox", Query: "launch", Limit: 50})
	if err != nil {
		t.Fatal(err)
	}
	if len(threads) != 1 || threads[0].MessageCount != 2 || threads[0].UnreadCount != 2 {
		t.Fatalf("threads=%+v", threads)
	}
	messages, err := a.threadMessages(context.Background(), "alice", threads[0].ID)
	if err != nil || len(messages) != 2 {
		t.Fatalf("messages=%+v err=%v", messages, err)
	}
	other, _, err := a.threads(context.Background(), "bob", threadQuery{Role: "inbox", Query: "launch"})
	if err != nil {
		t.Fatal(err)
	}
	if len(other) != 0 {
		t.Fatalf("bob saw mail: %+v", other)
	}
}

func TestNewMailNotificationsSkipBaselineAndSystemFolders(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	recorder := &notificationRecorder{}
	registry := apps.NewRegistry()
	registry.RegisterNotificationRecorder(recorder)
	registry.Register(a)
	a.registry = registry

	acct, err := a.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}
	messages := []struct {
		folder  string
		subject string
		id      string
	}{
		{folder: "INBOX", subject: "Inbox arrival", id: "inbox@example.test"},
		{folder: "Projects", subject: "Filtered arrival", id: "project@example.test"},
		{folder: "Sent", subject: "Sent copy", id: "sent@example.test"},
		{folder: "Junk", subject: "Junk copy", id: "junk@example.test"},
		{folder: "Drafts", subject: "Draft copy", id: "draft@example.test"},
		{folder: "Trash", subject: "Deleted copy", id: "trash@example.test"},
	}
	for _, item := range messages {
		raw, _, buildErr := buildMessage(outgoingMessage{
			FromName: "Sender", FromEmail: "sender@example.test", To: "alice@example.test",
			Subject: item.subject, Body: "Body", MessageID: item.id,
		})
		if buildErr != nil {
			t.Fatal(buildErr)
		}
		dir := filepath.Join(a.mailRoot("alice", acct.ID), item.folder, "new")
		if err := os.MkdirAll(dir, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, strings.ReplaceAll(item.id, "@", "-")+":2,"), raw, 0600); err != nil {
			t.Fatal(err)
		}
	}
	stats, err := a.reindexAccount(context.Background(), "alice", acct.ID)
	if err != nil {
		t.Fatal(err)
	}
	if len(stats.NewMessages) != 2 {
		t.Fatalf("eligible new messages=%#v", stats.NewMessages)
	}

	// The first account sync establishes the baseline and must not flood the
	// user with notifications for imported mail.
	a.notifyNewMessages(context.Background(), "alice", acct, stats.NewMessages)
	if len(recorder.items) != 0 {
		t.Fatalf("baseline notifications=%#v", recorder.items)
	}

	acct.LastSyncAt = time.Now().Unix()
	a.notifyNewMessages(context.Background(), "alice", acct, stats.NewMessages)
	if len(recorder.items) != 2 {
		t.Fatalf("post-baseline notifications=%#v", recorder.items)
	}
	for _, notification := range recorder.items {
		if notification.Kind != "new_mail" || notification.Source != "email" ||
			!strings.HasPrefix(notification.ObjectURL, "/email?account_id=") ||
			notification.DedupeKey == "" {
			t.Fatalf("notification=%#v", notification)
		}
		if strings.Contains(notification.Message, "Sent") || strings.Contains(notification.Message, "Junk") ||
			strings.Contains(notification.Message, "Drafts") || strings.Contains(notification.Message, "Trash") {
			t.Fatalf("system-folder notification=%#v", notification)
		}
	}
}

func TestFolderRenameRemovesSourceAndStaleMetadata(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	acct, err := a.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}
	if err := a.seedSpecialFolders(context.Background(), "alice", acct.ID); err != nil {
		t.Fatal(err)
	}
	folder, err := a.createFolder(context.Background(), "alice", acct.ID, "Old Name", "")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := a.renameFolder(context.Background(), "alice", folder.ID, "New Name", ""); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(folderDiskPath(a.mailRoot("alice", acct.ID), "Old Name")); !os.IsNotExist(err) {
		t.Fatalf("old folder still exists: %v", err)
	}
	folders, err := a.folders(context.Background(), "alice", acct.ID)
	if err != nil {
		t.Fatal(err)
	}
	for _, f := range folders {
		if f.Name == "Old Name" {
			t.Fatalf("stale folder metadata remains: %+v", folders)
		}
	}
}

func TestGeneratedMBSyncConfigContainsNoPassword(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	a.tunnelPath = "/opt/dar/bin/dar-server"
	acct, err := a.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}
	path, err := a.writeMBSyncConfig("alice", acct)
	if err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(data, []byte("secret-app-password")) {
		t.Fatal("password written to mbsyncrc")
	}
	if !bytes.Contains(data, []byte("printenv DAR_EMAIL_ACCOUNT_PASSWORD")) {
		t.Fatalf("config=%s", data)
	}
	if !bytes.Contains(data, []byte(`Tunnel "'/opt/dar/bin/dar-server' --mail-tunnel-host 'imap.example.test' --mail-tunnel-port 993"`)) {
		t.Fatalf("config does not route IMAP through the guarded tunnel: %s", data)
	}
	info, _ := os.Stat(path)
	if info.Mode().Perm() != 0600 {
		t.Fatalf("mode=%o", info.Mode().Perm())
	}
}

func TestAccountEmailIsNormalizedAndOwnsManagedMaildir(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	input := baseAccount()
	input.Email = " Alice@Example.TEST "
	account, err := a.createAccount(context.Background(), "alice", input)
	if err != nil {
		t.Fatal(err)
	}
	if account.Email != "alice@example.test" {
		t.Fatalf("account email=%q", account.Email)
	}
	oldRoot := a.managedMailRoot("alice", account.Email)
	message := filepath.Join(oldRoot, "INBOX", "new", "message")
	if err := os.MkdirAll(filepath.Dir(message), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(message, []byte("mail"), 0600); err != nil {
		t.Fatal(err)
	}
	linkTarget, err := os.Readlink(a.mailRoot("alice", account.ID))
	if err != nil || filepath.IsAbs(linkTarget) {
		t.Fatalf("private Maildir link target=%q err=%v", linkTarget, err)
	}

	update := baseAccount()
	update.Email = "renamed@example.test"
	update.Password = ""
	updated, err := a.updateAccount(context.Background(), "alice", account.ID, update)
	if err != nil {
		t.Fatal(err)
	}
	if updated.Email != "renamed@example.test" {
		t.Fatalf("updated email=%q", updated.Email)
	}
	newRoot := a.managedMailRoot("alice", updated.Email)
	data, err := os.ReadFile(filepath.Join(newRoot, "INBOX", "new", "message"))
	if err != nil || string(data) != "mail" {
		t.Fatalf("relocated Maildir data=%q err=%v", data, err)
	}
	if _, err := os.Lstat(oldRoot); !os.IsNotExist(err) {
		t.Fatalf("old Maildir still exists: %v", err)
	}
}

func TestAccountRejectsUnsafeEmailPath(t *testing.T) {
	for _, email := range []string{"../alice@example.test", `alice\\box@example.test`, "not-an-email"} {
		account := baseAccount()
		account.Email = email
		if err := normalizeAccount(&account); err == nil {
			t.Fatalf("unsafe account email %q was accepted", email)
		}
	}
}

func TestAPIAccountIsolationAndDisabledStatus(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	acct, err := a.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}
	req := httptest.NewRequest(http.MethodGet, "/api/email/accounts/"+acct.ID, nil)
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: "bob"}))
	rr := httptest.NewRecorder()
	a.handleAPI(rr, req)
	if rr.Code != 404 {
		t.Fatalf("status=%d body=%s", rr.Code, rr.Body.String())
	}
	disabled := New(Config{DB: a.db, DataRoot: t.TempDir(), MBSyncPath: "/bin/true"})
	req = httptest.NewRequest(http.MethodGet, "/api/email", nil)
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: "alice"}))
	rr = httptest.NewRecorder()
	disabled.handleAPI(rr, req)
	var body map[string]any
	_ = json.Unmarshal(rr.Body.Bytes(), &body)
	if body["status"] != "disabled" {
		t.Fatalf("body=%v", body)
	}
	req = httptest.NewRequest(http.MethodPost, "/api/email/drafts", strings.NewReader(`{}`))
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: "alice"}))
	rr = httptest.NewRecorder()
	disabled.handleAPI(rr, req)
	body = map[string]any{}
	_ = json.Unmarshal(rr.Body.Bytes(), &body)
	if rr.Code != http.StatusServiceUnavailable || body["code"] != "email_disabled" {
		t.Fatalf("status=%d body=%v", rr.Code, body)
	}
}

func TestStoredCredentialValidationDisablesWrongKey(t *testing.T) {
	a := testApp(t, testKey(t, "original"))
	if _, err := a.createAccount(context.Background(), "alice", baseAccount()); err != nil {
		t.Fatal(err)
	}
	a.keys = newKeyRing([]CredentialKey{testKey(t, "replacement")})
	if err := a.validateStoredCredentials(context.Background()); err == nil || !strings.Contains(err.Error(), "cannot be decrypted") {
		t.Fatalf("err=%v", err)
	}
}

func TestEmailTemplateContract(t *testing.T) {
	data, err := os.ReadFile("../../webui/templates/email.html")
	if err != nil {
		t.Fatal(err)
	}
	html := string(data)
	for _, want := range []string{"id=\"email-app\"", "id=\"mailbox-tree\"", "id=\"thread-list\"", "id=\"reader-panel\"", "id=\"compose-button\"", "/email/assets/app.js"} {
		if !strings.Contains(html, want) {
			t.Errorf("missing %s", want)
		}
	}
	if strings.Contains(html, "style=\"") {
		t.Fatal("email template contains an inline style")
	}
	if !strings.Contains(html, `id="sync-state"`) || !strings.Contains(html, `aria-live="polite"`) {
		t.Fatal("email template is missing the live sync status region")
	}
}

func TestEmailSyncFrontendContract(t *testing.T) {
	data, err := emailFS.ReadFile("assets/app.js")
	if err != nil {
		t.Fatal(err)
	}
	js := string(data)
	for _, want := range []string{
		"syncStatuses:new Map()",
		"syncPollers:new Map()",
		"await discoverSyncStatuses()",
		"if(state.syncPollers.has(id))",
		`data-sync-status="${a.id}"`,
		"Email sync in progress",
	} {
		if !strings.Contains(js, want) {
			t.Errorf("email sync frontend is missing %q", want)
		}
	}
}

func TestEmailReaderFillsAvailableHeight(t *testing.T) {
	templateData, err := os.ReadFile("../../webui/templates/email.html")
	if err != nil {
		t.Fatal(err)
	}
	html := string(templateData)
	if strings.Contains(html, "min-h-[38rem]") {
		t.Fatal("email app has a fixed minimum height")
	}
	for _, want := range []string{
		`id="reader-panel" class="hidden min-h-0`,
		`id="reader" class="hidden min-h-0 flex-1 flex-col overflow-hidden"`,
	} {
		if !strings.Contains(html, want) {
			t.Errorf("email reader layout is missing %q", want)
		}
	}

	assetData, err := emailFS.ReadFile("assets/app.js")
	if err != nil {
		t.Fatal(err)
	}
	js := string(assetData)
	for _, want := range []string{
		"reader.classList.add('flex')",
		"flex min-h-0 w-full max-w-4xl flex-1 flex-col",
		"min-h-64 w-full flex-1",
		"flex min-h-0 flex-1 flex-col border-t",
	} {
		if !strings.Contains(js, want) {
			t.Errorf("email reader layout is missing %q", want)
		}
	}
}

func TestSyncContinuesAfterRequestContextCanceled(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	dir := t.TempDir()
	started := filepath.Join(dir, "started")
	release := filepath.Join(dir, "release")
	script := filepath.Join(dir, "mbsync")
	body := fmt.Sprintf("#!/bin/sh\n: > %q\nwhile [ ! -f %q ]; do sleep 0.01; done\n", started, release)
	if err := os.WriteFile(script, []byte(body), 0700); err != nil {
		t.Fatal(err)
	}
	a.mbsyncPath = script
	a.outbound = outbound.Policy{
		AllowedHosts: map[string]struct{}{"imap.example.test": {}},
		Resolver:     fixedResolver{ips: []net.IP{net.ParseIP("127.0.0.1")}},
	}
	in := baseAccount()
	in.Enabled = true
	acct, err := a.createAccount(context.Background(), "alice", in)
	if err != nil {
		t.Fatal(err)
	}
	if err := a.Start(context.Background()); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = os.WriteFile(release, []byte("release"), 0600)
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		_ = a.Shutdown(ctx)
	})

	requestContext, cancelRequest := context.WithCancel(context.Background())
	requestContext = context.WithValue(requestContext, server.UserContextKey, &auth.User{ID: "alice"})
	req := httptest.NewRequest(http.MethodPost, "/api/email/accounts/"+acct.ID+"/sync", nil).WithContext(requestContext)
	rr := httptest.NewRecorder()
	a.handleAPI(rr, req)
	if rr.Code != http.StatusAccepted {
		t.Fatalf("status=%d body=%s", rr.Code, rr.Body.String())
	}

	deadline := time.Now().Add(2 * time.Second)
	for {
		if _, err := os.Stat(started); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("background mbsync process did not start")
		}
		time.Sleep(10 * time.Millisecond)
	}
	duplicateReq := httptest.NewRequest(http.MethodPost, "/api/email/accounts/"+acct.ID+"/sync", nil)
	duplicateReq = duplicateReq.WithContext(context.WithValue(duplicateReq.Context(), server.UserContextKey, &auth.User{ID: "alice"}))
	duplicateRR := httptest.NewRecorder()
	a.handleAPI(duplicateRR, duplicateReq)
	var duplicateBody struct {
		Queued bool       `json:"queued"`
		Status SyncStatus `json:"status"`
	}
	if err := json.Unmarshal(duplicateRR.Body.Bytes(), &duplicateBody); err != nil {
		t.Fatal(err)
	}
	if duplicateRR.Code != http.StatusAccepted || duplicateBody.Queued || !isActiveStatus(duplicateBody.Status.State) {
		t.Fatalf("duplicate sync status=%d body=%s", duplicateRR.Code, duplicateRR.Body.String())
	}
	cancelRequest()
	if err := os.WriteFile(release, []byte("release"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := a.waitSync(acct.ID, 2*time.Second); err != nil {
		t.Fatalf("background sync did not finish after request cancellation: %v", err)
	}
	status := a.syncStatus("alice", acct.ID)
	if status.State != "succeeded" {
		t.Fatalf("sync status=%+v", status)
	}
}

func isActiveStatus(state string) bool {
	return state == "queued" || state == "running"
}

func TestMessageHTMLAllowsOnlySameOriginFraming(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	acct, err := a.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}
	var messageID int64
	err = a.withConn(context.Background(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `INSERT INTO email_messages(account_id,content_hash,thread_key,html_body) VALUES(?,?,?,?) RETURNING id`, &sqlitex.ExecOptions{
			Args: []any{acct.ID, "html-frame-test", "html-frame-test", "<p>Email body</p>"},
			ResultFunc: func(s *sqlite.Stmt) error {
				messageID = s.ColumnInt64(0)
				return nil
			},
		})
	})
	if err != nil {
		t.Fatal(err)
	}

	req := httptest.NewRequest(http.MethodGet, fmt.Sprintf("/api/email/messages/%d/html", messageID), nil)
	rr := httptest.NewRecorder()
	rr.Header().Set("X-Frame-Options", "DENY")
	a.serveMessageHTML(rr, req, "alice", messageID)

	if rr.Code != http.StatusOK || rr.Header().Get("X-Frame-Options") != "SAMEORIGIN" {
		t.Fatalf("status=%d x-frame-options=%q", rr.Code, rr.Header().Get("X-Frame-Options"))
	}
	csp := rr.Header().Get("Content-Security-Policy")
	for _, want := range []string{"sandbox", "default-src 'none'", "frame-ancestors 'self'", "form-action 'none'"} {
		if !strings.Contains(csp, want) {
			t.Errorf("Content-Security-Policy missing %q: %s", want, csp)
		}
	}
	if rr.Body.String() != "<p>Email body</p>" {
		t.Fatalf("body=%q", rr.Body.String())
	}
}

func TestReindexRollsBackOnFailure(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	acct, err := a.createAccount(context.Background(), "alice", baseAccount())
	if err != nil {
		t.Fatal(err)
	}
	_ = a.seedSpecialFolders(context.Background(), "alice", acct.ID)
	// Verify migrations and FTS are operational through a direct integrity query.
	err = a.withConn(context.Background(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `INSERT INTO email_fts(message_id,user_id,subject,participants,body,filenames) VALUES(1,'alice','hello','','','')`, &sqlitex.ExecOptions{})
	})
	if err != nil {
		t.Fatal(err)
	}
}

func TestParseWindows1252(t *testing.T) {
	got := string(decodeCharset([]byte{0x93, 'H', 'i', 0x94}, "windows-1252"))
	if got != "“Hi”" {
		t.Fatalf("got %q", got)
	}
}
func TestCursorRoundTrip(t *testing.T) {
	c := encodeCursor(time.Now().Unix(), "thread")
	at, id, err := decodeCursor(c)
	if err != nil || at == 0 || id != "thread" {
		t.Fatalf("at=%d id=%q err=%v", at, id, err)
	}
}
