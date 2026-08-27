package email

import (
	"bytes"
	"context"
	"crypto/tls"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/mail"
	"net/smtp"
	"net/textproto"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/outbound"
)

func TestEmailDiscoveryFormattingAndBufferBoundaries(t *testing.T) {
	for _, tc := range []struct {
		socket string
		port   int
		want   securityMode
	}{
		{"SSL", 1, securityTLS},
		{"SSL/TLS", 1, securityTLS},
		{"IMAPS", 1, securityTLS},
		{"STARTTLS", 1, securitySTARTTLS},
		{"", 993, securityTLS},
		{"", 465, securityTLS},
		{"", 143, securitySTARTTLS},
		{"", 587, securitySTARTTLS},
		{"", 2525, securityNone},
	} {
		if got := socketMode(tc.socket, tc.port); got != tc.want {
			t.Errorf("socketMode(%q,%d)=%q want=%q", tc.socket, tc.port, got, tc.want)
		}
	}
	if parsePort("2525", 25) != 2525 || parsePort("bad", 25) != 25 || parsePort("-1", 25) != 25 {
		t.Fatal("parsePort regression")
	}
	if got := stripHTML("<p>Hello <strong>world</strong></p>"); strings.Contains(got, "<") || !strings.Contains(got, "Hello") {
		t.Fatalf("stripHTML=%q", got)
	}
	if decodeHeader("=?UTF-8?Q?Hello_=E2=9C=93?=") != "Hello ✓" ||
		!strings.Contains(decodeHeader("=?ISO-8859-1?Q?Andr=E9?="), "Andr") ||
		decodeHeader(" plain ") != "plain" {
		t.Fatal("header decoding regression")
	}
	if string(decodeCharset([]byte{0x80, 'A'}, "windows-1252")) != "€A" ||
		string(decodeCharset([]byte{0xe9}, "latin1")) != "é" ||
		len(decodeCharset([]byte{0xff}, "utf-8")) == 0 ||
		len(decodeCharset([]byte{0xff}, "unknown")) == 0 {
		t.Fatal("charset decoding regression")
	}
	now := time.Now()
	for _, tc := range []struct {
		ts     int64
		needle string
	}{
		{0, ""},
		{now.Add(-10 * time.Second).Unix(), "now"},
		{now.Add(-5 * time.Minute).Unix(), "5m"},
		{now.Add(-2 * time.Hour).Unix(), "2h"},
		{now.Add(-2 * 24 * time.Hour).Unix(), "2d"},
	} {
		if got := relativeMailTime(tc.ts); got != tc.needle {
			t.Errorf("relativeMailTime(%d)=%q want=%q", tc.ts, got, tc.needle)
		}
	}
	if relativeMailTime(now.Add(-8*24*time.Hour).Unix()) == "" ||
		messageDomain("alice@example.test") != "example.test" || messageDomain("local") != "dar.local" ||
		!isLoopbackHost("localhost") || !isLoopbackHost("127.0.0.1") || isLoopbackHost("example.test") {
		t.Fatal("mail helper regression")
	}

	suggestions := []struct{ Name, Email string }{{"Z", "z@example.test"}, {"A", "A@example.test"}}
	sortSuggestions(suggestions)
	if suggestions[0].Email != "A@example.test" {
		t.Fatalf("suggestions=%v", suggestions)
	}

	var limited limitedBuffer
	large := bytes.Repeat([]byte("x"), (32<<10)+100)
	n, err := limited.Write(large)
	if err != nil || n != 32<<10 || limited.Len() != 32<<10 {
		t.Fatalf("first limited write n=%d len=%d err=%v", n, limited.Len(), err)
	}
	n, err = limited.Write([]byte("ignored"))
	if err != nil || n != len("ignored") || limited.Len() != 32<<10 {
		t.Fatalf("second limited write n=%d len=%d err=%v", n, limited.Len(), err)
	}
	if wrapDBErr("load", nil) != nil || !strings.Contains(wrapDBErr("load", errors.New("boom")).Error(), "load: boom") {
		t.Fatal("wrapDBErr regression")
	}

	var encoded bytes.Buffer
	if err := writeBase64Lines(&encoded, bytes.Repeat([]byte{0x42}, 100)); err != nil {
		t.Fatal(err)
	}
	for _, line := range strings.Split(strings.TrimSpace(encoded.String()), "\r\n") {
		if len(line) > 76 {
			t.Fatalf("base64 line too long: %d", len(line))
		}
	}
	if formatMessageID("id@example.test") != "<id@example.test>" || formatMessageID("<id@example.test>") != "<id@example.test>" {
		t.Fatal("message ID formatting regression")
	}
}

func TestEmailFetchAutoResponsesAndDiscoverValidation(t *testing.T) {
	const config = `<clientConfig><emailProvider><incomingServer type="imap"><hostname>imap.example.test</hostname><port>993</port><socketType>SSL</socketType><username>%EMAILADDRESS%</username></incomingServer><outgoingServer type="smtp"><hostname>smtp.example.test</hostname><port>587</port><socketType>STARTTLS</socketType><username>%EMAILLOCALPART%</username></outgoingServer></emailProvider></clientConfig>`
	fixture := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/ok":
			_, _ = w.Write([]byte(config))
		case "/large":
			_, _ = w.Write(bytes.Repeat([]byte("x"), (1<<20)+1))
		case "/bad":
			_, _ = w.Write([]byte("<invalid"))
		default:
			http.Error(w, "missing", http.StatusNotFound)
		}
	}))
	defer fixture.Close()
	policy, err := outbound.ParsePolicy("127.0.0.1", "")
	if err != nil {
		t.Fatal(err)
	}
	result, err := fetchAuto(context.Background(), fixture.URL+"/ok", "alice@example.test", "fixture", policy)
	if err != nil || result.IMAPHost != "imap.example.test" || result.Username != "alice@example.test" {
		t.Fatalf("fetch result=%+v err=%v", result, err)
	}
	for _, path := range []string{"/missing", "/large", "/bad"} {
		if _, err := fetchAuto(context.Background(), fixture.URL+path, "alice@example.test", "fixture", policy); err == nil {
			t.Errorf("%s response accepted", path)
		}
	}
	denied, err := outbound.ParsePolicy("example.test", "")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := fetchAuto(context.Background(), fixture.URL+"/ok", "alice@example.test", "fixture", denied); err == nil {
		t.Fatal("outbound policy bypassed")
	}
	if _, err := discoverAccount(context.Background(), "not-an-email", policy); err == nil {
		t.Fatal("invalid discovery email accepted")
	}

	a := testApp(t, testKey(t, "current"))
	for _, tc := range []struct {
		method string
		body   string
		status int
	}{
		{http.MethodGet, `{}`, http.StatusMethodNotAllowed},
		{http.MethodPost, `{`, http.StatusBadRequest},
		{http.MethodPost, `{"email":"invalid"}`, http.StatusBadRequest},
	} {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(tc.method, "/api/email/discover", strings.NewReader(tc.body))
		a.apiDiscover(rec, req)
		if rec.Code != tc.status {
			t.Fatalf("%s discover status=%d body=%s", tc.method, rec.Code, rec.Body.String())
		}
	}
}

func TestEmailClosedDatabaseAndRoutingErrorMatrix(t *testing.T) {
	app := testApp(t, testKey(t, "closed-db"))
	if err := app.db.Close(); err != nil {
		t.Fatal(err)
	}
	cases := []struct {
		method string
		target string
		body   any
	}{
		{http.MethodGet, "/api/email/accounts", nil},
		{http.MethodPost, "/api/email/accounts", Account{Email: "alice@example.test"}},
		{http.MethodGet, "/api/email/accounts/missing", nil},
		{http.MethodPatch, "/api/email/accounts/missing", Account{Email: "alice@example.test"}},
		{http.MethodDelete, "/api/email/accounts/missing", nil},
		{http.MethodPost, "/api/email/accounts/missing/test", Account{}},
		{http.MethodGet, "/api/email/accounts/missing/sync", nil},
		{http.MethodPost, "/api/email/accounts/missing/sync", nil},
		{http.MethodGet, "/api/email/accounts/missing/folders", nil},
		{http.MethodPost, "/api/email/accounts/missing/folders", map[string]string{"Name": "Folder"}},
		{http.MethodPatch, "/api/email/accounts/missing/folders/1", map[string]string{"Name": "Folder"}},
		{http.MethodDelete, "/api/email/accounts/missing/folders/1", nil},
		{http.MethodGet, "/api/email/mailboxes", nil},
		{http.MethodGet, "/api/email/threads", nil},
		{http.MethodGet, "/api/email/threads/thread", nil},
		{http.MethodPost, "/api/email/messages/actions", map[string]any{"action": "read", "message_ids": []int64{1}}},
		{http.MethodGet, "/api/email/messages/1/html", nil},
		{http.MethodGet, "/api/email/messages/1/attachments/1", nil},
		{http.MethodPost, "/api/email/drafts", DraftInput{AccountID: "missing", Subject: "Draft"}},
		{http.MethodGet, "/api/email/drafts/1", nil},
		{http.MethodPatch, "/api/email/drafts/1", DraftInput{Subject: "Draft"}},
		{http.MethodDelete, "/api/email/drafts/1", nil},
		{http.MethodPost, "/api/email/drafts/1/send", nil},
		{http.MethodPost, "/api/email/drafts/1/attachments", nil},
		{http.MethodGet, "/api/email/preferences", nil},
		{http.MethodPut, "/api/email/preferences", Preferences{}},
		{http.MethodGet, "/api/email/recipients", nil},
	}
	for _, tc := range cases {
		rec := emailAPIRequest(t, app, "alice", tc.method, tc.target, tc.body)
		if rec.Code < 400 && tc.target != "/api/email/recipients" {
			t.Errorf("%s %s unexpectedly returned %d: %s", tc.method, tc.target, rec.Code, rec.Body.String())
		}
	}
	for _, err := range []error{
		errNotFound,
		errors.New("account disabled"),
		errors.New("encryption unavailable"),
		errors.New("operation not supported"),
		errors.New("folder not empty"),
		errors.New("connection test failed"),
		errors.New("invalid"),
	} {
		status, code := statusForError(err)
		if status == 0 || code == "" {
			t.Fatalf("unclassified error %v", err)
		}
	}
}

func TestEmailManagedMailRootAndMIMEHelperEdges(t *testing.T) {
	override := New(Config{DataRoot: t.TempDir()})
	overridePath := filepath.Join(t.TempDir(), "override")
	if got, err := override.ensureManagedMailRoot("alice", Account{mailRootOverride: overridePath}); err != nil || got != overridePath {
		t.Fatalf("mail override=%q err=%v", got, err)
	}
	blocker := filepath.Join(t.TempDir(), "blocker")
	if err := os.WriteFile(blocker, []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := override.ensureManagedMailRoot("alice", Account{mailRootOverride: filepath.Join(blocker, "child")}); err == nil {
		t.Fatal("invalid mail override succeeded")
	}

	for name, setup := range map[string]func(*App, Account) error{
		"escaping symlink": func(app *App, account Account) error {
			link := app.mailRoot("alice", account.ID)
			if err := os.MkdirAll(filepath.Dir(link), 0o700); err != nil {
				return err
			}
			return os.Symlink("/tmp", link)
		},
		"regular legacy path": func(app *App, account Account) error {
			link := app.mailRoot("alice", account.ID)
			if err := os.MkdirAll(filepath.Dir(link), 0o700); err != nil {
				return err
			}
			return os.WriteFile(link, []byte("not maildir"), 0o600)
		},
	} {
		app := New(Config{DataRoot: t.TempDir()})
		account := Account{ID: "account", Email: "alice@example.test"}
		if err := setup(app, account); err != nil {
			t.Fatal(err)
		}
		if _, err := app.ensureManagedMailRoot("alice", account); err == nil {
			t.Errorf("%s unexpectedly succeeded", name)
		}
	}

	legacy := New(Config{DataRoot: t.TempDir()})
	account := Account{ID: "legacy", Email: "legacy@example.test"}
	if err := os.MkdirAll(legacy.mailRoot("alice", account.ID), 0o700); err != nil {
		t.Fatal(err)
	}
	target, err := legacy.ensureManagedMailRoot("alice", account)
	if err != nil {
		t.Fatal(err)
	}
	if info, err := os.Lstat(legacy.mailRoot("alice", account.ID)); err != nil || info.Mode()&os.ModeSymlink == 0 {
		t.Fatalf("legacy link info=%v err=%v target=%q", info, err, target)
	}
	if got, err := legacy.ensureManagedMailRoot("alice", account); err != nil || got != target {
		t.Fatalf("existing managed mail root=%q err=%v", got, err)
	}

	for _, value := range []string{
		"=?UTF-8?Q?Hello_=E2=9C=93?=",
		"=?ISO-8859-1?Q?Andr=E9?=",
		"=?bad?Q?broken?=",
	} {
		if decodeHeader(value) == "" {
			t.Errorf("empty decoded header for %q", value)
		}
	}
	header := make(mail.Header)
	header["To"] = []string{"not a valid address,"}
	if recipients, label := parseAddresses("To", header, 0); len(recipients) != 0 || label == "" {
		t.Fatalf("malformed address result=%v %q", recipients, label)
	}
	if recipients, label := parseAddresses("Cc", header, 0); len(recipients) != 0 || label != "" {
		t.Fatalf("empty address result=%v %q", recipients, label)
	}
}

func TestEmailDiscoveryProviderSRVThunderbirdAndHeuristicPaths(t *testing.T) {
	policy, err := outbound.ParsePolicy("", "")
	if err != nil {
		t.Fatal(err)
	}
	providerResult := DiscoveryResult{Email: "alice@example.test", IMAPHost: "provider-imap", Source: "provider"}
	calls := 0
	deps := discoveryDeps{
		fetch: func(context.Context, string, string, string, outbound.Policy) (DiscoveryResult, error) {
			calls++
			if calls == 2 {
				return providerResult, nil
			}
			return DiscoveryResult{}, errors.New("missing")
		},
		lookupSRV: func(context.Context, string, string, string) (string, []*net.SRV, error) {
			return "", nil, nil
		},
	}
	if got, err := discoverAccountWithDeps(context.Background(), " Alice@Example.Test ", deps, policy); err != nil || got.IMAPHost != "provider-imap" || calls != 2 {
		t.Fatalf("provider discovery=%+v calls=%d err=%v", got, calls, err)
	}

	failingFetch := func(context.Context, string, string, string, outbound.Policy) (DiscoveryResult, error) {
		return DiscoveryResult{}, errors.New("missing")
	}
	deps.fetch = failingFetch
	deps.lookupSRV = func(_ context.Context, service, _, _ string) (string, []*net.SRV, error) {
		switch service {
		case "imaps":
			return "", []*net.SRV{{Target: "imap.srv.test.", Port: 1993}}, nil
		case "submissions":
			return "", []*net.SRV{{Target: "smtp.srv.test.", Port: 1465}}, nil
		default:
			return "", nil, nil
		}
	}
	got, err := discoverAccountWithDeps(context.Background(), "alice@example.test", deps, policy)
	if err != nil || got.Source != "dns-srv" || got.IMAPSecurity != securityTLS || got.SMTPSecurity != securityTLS ||
		got.IMAPHost != "imap.srv.test" || got.SMTPHost != "smtp.srv.test" {
		t.Fatalf("TLS SRV discovery=%+v err=%v", got, err)
	}

	deps.lookupSRV = func(_ context.Context, service, _, _ string) (string, []*net.SRV, error) {
		switch service {
		case "imap":
			return "", []*net.SRV{{Target: "imap-starttls.test.", Port: 1143}}, nil
		case "submission":
			return "", []*net.SRV{{Target: "smtp-starttls.test.", Port: 1587}}, nil
		default:
			return "", nil, nil
		}
	}
	got, err = discoverAccountWithDeps(context.Background(), "alice@example.test", deps, policy)
	if err != nil || got.IMAPSecurity != securitySTARTTLS || got.SMTPSecurity != securitySTARTTLS {
		t.Fatalf("STARTTLS SRV discovery=%+v err=%v", got, err)
	}

	deps.lookupSRV = func(context.Context, string, string, string) (string, []*net.SRV, error) {
		return "", nil, nil
	}
	fetchCalls := 0
	deps.fetch = func(_ context.Context, _ string, email, source string, _ outbound.Policy) (DiscoveryResult, error) {
		fetchCalls++
		if source == "thunderbird-ispdb" {
			return DiscoveryResult{Email: email, Source: source, IMAPHost: "ispdb-imap"}, nil
		}
		return DiscoveryResult{}, errors.New("missing")
	}
	got, err = discoverAccountWithDeps(context.Background(), "alice@example.test", deps, policy)
	if err != nil || got.Source != "thunderbird-ispdb" || fetchCalls != 3 {
		t.Fatalf("ISPDB discovery=%+v calls=%d err=%v", got, fetchCalls, err)
	}

	deps.fetch = failingFetch
	got, err = discoverAccountWithDeps(context.Background(), "alice@example.test", deps, policy)
	if err != nil || got.Source != "heuristic" || got.IMAPHost != "imap.example.test" || got.SMTPPort != 587 || len(got.Candidates) != 2 {
		t.Fatalf("heuristic discovery=%+v err=%v", got, err)
	}
	if _, err := discoverAccountWithDeps(context.Background(), "invalid", deps, policy); err == nil {
		t.Fatal("invalid email accepted by injected discovery")
	}
}

func TestEmailDueAccountsExtractionAndAccountTestValidation(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	due := baseAccount()
	due.Enabled = true
	due.SyncMinutes = 1
	created, err := a.createAccount(context.Background(), "alice", due)
	if err != nil {
		t.Fatal(err)
	}
	disabled := baseAccount()
	disabled.Email = "disabled@example.test"
	disabled.Username = disabled.Email
	disabled.Enabled = false
	if _, err := a.createAccount(context.Background(), "alice", disabled); err != nil {
		t.Fatal(err)
	}
	owners, err := a.dueAccountIDs(context.Background(), time.Now().Unix())
	if err != nil || len(owners) != 1 || owners[0].AccountID != created.ID || owners[0].UserID != "alice" {
		t.Fatalf("due accounts=%v err=%v", owners, err)
	}

	if _, err := a.testAccount(context.Background(), "alice", Account{}); err == nil {
		t.Fatal("invalid test account accepted")
	}
	noPassword := baseAccount()
	noPassword.Password = ""
	if _, err := a.testAccount(context.Background(), "alice", noPassword); err == nil {
		t.Fatal("missing password accepted")
	}
	unavailable := testApp(t)
	if _, err := unavailable.testAccount(context.Background(), "alice", baseAccount()); err == nil {
		t.Fatal("unavailable service accepted account test")
	}

	path := filepath.Join(t.TempDir(), "message.eml")
	if err := os.WriteFile(path, []byte("From: a@example.test\r\nTo: b@example.test\r\nSubject: x\r\nContent-Type: text/plain\r\n\r\nbody"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := extractPart(path, "missing"); !errors.Is(err, errNotFound) {
		t.Fatalf("missing attachment err=%v", err)
	}
	if _, err := extractPart(filepath.Join(t.TempDir(), "missing.eml"), "1"); err == nil {
		t.Fatal("missing message path accepted")
	}
}

func TestEmailAccountAPIBranchMatrix(t *testing.T) {
	a := testApp(t, testKey(t, "current"))
	connectionFailure := baseAccount()
	connectionFailure.Enabled = true
	connectionFailure.IMAPHost = "127.0.0.1"
	connectionFailure.IMAPPort = 1
	connectionFailure.SMTPHost = "127.0.0.1"
	connectionFailure.SMTPPort = 1
	rec := emailAPIRequest(t, a, "alice", http.MethodPost, "/api/email/accounts", connectionFailure)
	if rec.Code != http.StatusUnprocessableEntity {
		t.Fatalf("enabled account connection failure status=%d body=%s", rec.Code, rec.Body.String())
	}
	accountInput := baseAccount()
	accountInput.Enabled = false
	rec = emailAPIRequest(t, a, "alice", http.MethodPost, "/api/email/accounts", accountInput)
	if rec.Code != http.StatusCreated {
		t.Fatalf("create status=%d body=%s", rec.Code, rec.Body.String())
	}
	var account Account
	if err := json.Unmarshal(rec.Body.Bytes(), &account); err != nil {
		t.Fatal(err)
	}

	for _, target := range []string{"/api/email/accounts", "/api/email/accounts/" + account.ID, "/api/email/accounts/" + account.ID + "/folders", "/api/email/mailboxes?account_id=" + account.ID} {
		rec = emailAPIRequest(t, a, "alice", http.MethodGet, target, nil)
		if rec.Code != http.StatusOK {
			t.Fatalf("GET %s status=%d body=%s", target, rec.Code, rec.Body.String())
		}
	}

	accountInput.Name = "Updated"
	accountInput.Password = ""
	rec = emailAPIRequest(t, a, "alice", http.MethodPatch, "/api/email/accounts/"+account.ID, accountInput)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"name":"Updated"`) {
		t.Fatalf("patch status=%d body=%s", rec.Code, rec.Body.String())
	}
	connectionFailure.ID = account.ID
	connectionFailure.Password = ""
	rec = emailAPIRequest(t, a, "alice", http.MethodPatch, "/api/email/accounts/"+account.ID, connectionFailure)
	if rec.Code != http.StatusUnprocessableEntity {
		t.Fatalf("enabled patch connection failure status=%d body=%s", rec.Code, rec.Body.String())
	}

	for _, tc := range []struct {
		method string
		target string
		body   any
		status int
	}{
		{http.MethodPut, "/api/email/accounts", nil, http.StatusMethodNotAllowed},
		{http.MethodPost, "/api/email/accounts/test", Account{}, http.StatusUnprocessableEntity},
		{http.MethodGet, "/api/email/accounts/test", nil, http.StatusMethodNotAllowed},
		{http.MethodGet, "/api/email/accounts/missing", nil, http.StatusNotFound},
		{http.MethodPatch, "/api/email/accounts/missing", accountInput, http.StatusNotFound},
		{http.MethodDelete, "/api/email/accounts/missing", nil, http.StatusNotFound},
		{http.MethodPut, "/api/email/accounts/" + account.ID, nil, http.StatusMethodNotAllowed},
		{http.MethodGet, "/api/email/accounts/" + account.ID + "/test", nil, http.StatusMethodNotAllowed},
		{http.MethodPost, "/api/email/accounts/" + account.ID + "/test", map[string]any{}, http.StatusUnprocessableEntity},
		{http.MethodPost, "/api/email/accounts/missing/test", map[string]any{}, http.StatusNotFound},
		{http.MethodGet, "/api/email/accounts/" + account.ID + "/sync", nil, http.StatusOK},
		{http.MethodPost, "/api/email/accounts/" + account.ID + "/sync", nil, http.StatusAccepted},
		{http.MethodPatch, "/api/email/accounts/" + account.ID + "/sync", nil, http.StatusMethodNotAllowed},
		{http.MethodGet, "/api/email/accounts/" + account.ID + "/unknown", nil, http.StatusNotFound},
		{http.MethodPost, "/api/email/accounts/" + account.ID + "/folders", map[string]string{"Name": ""}, http.StatusBadRequest},
		{http.MethodGet, "/api/email/accounts/" + account.ID + "/folders/not-a-number", nil, http.StatusBadRequest},
	} {
		rec = emailAPIRequest(t, a, "alice", tc.method, tc.target, tc.body)
		if rec.Code != tc.status {
			t.Errorf("%s %s status=%d want=%d body=%s", tc.method, tc.target, rec.Code, tc.status, rec.Body.String())
		}
	}
}

type matrixRecipientProvider struct{}

func (matrixRecipientProvider) SuggestRecipients(context.Context, string, string, int) ([]apps.RecipientSuggestion, error) {
	return []apps.RecipientSuggestion{
		{Name: "Alice", Email: "alice@example.test"},
		{Name: "Duplicate", Email: "ALICE@example.test"},
		{Name: "Empty"},
		{Name: "Bob", Email: "bob@example.test"},
		{Name: "Carol", Email: "carol@example.test"},
	}, nil
}

func TestEmailMailboxThreadMessageDraftPreferenceValidationMatrix(t *testing.T) {
	a := testApp(t, testKey(t, "api-validation"))
	a.recipients = matrixRecipientProvider{}
	accountInput := baseAccount()
	accountInput.Enabled = false
	rec := emailAPIRequest(t, a, "alice", http.MethodPost, "/api/email/accounts", accountInput)
	if rec.Code != http.StatusCreated {
		t.Fatalf("create account status=%d body=%s", rec.Code, rec.Body.String())
	}
	var account Account
	if err := json.Unmarshal(rec.Body.Bytes(), &account); err != nil {
		t.Fatal(err)
	}
	rec = emailAPIRequest(t, a, "alice", http.MethodPost, "/api/email/drafts", DraftInput{
		AccountID: account.ID, To: "bob@example.test", Subject: "Validation", Body: "draft",
	})
	if rec.Code != http.StatusCreated {
		t.Fatalf("create draft status=%d body=%s", rec.Code, rec.Body.String())
	}
	var draft Message
	if err := json.Unmarshal(rec.Body.Bytes(), &draft); err != nil {
		t.Fatal(err)
	}
	id := strconv.FormatInt(draft.ID, 10)

	for _, tc := range []struct {
		name, method, target string
		body                 any
		want                 int
	}{
		{"mailboxes method", http.MethodPost, "/api/email/mailboxes", nil, http.StatusMethodNotAllowed},
		{"threads method", http.MethodPost, "/api/email/threads", nil, http.StatusMethodNotAllowed},
		{"threads invalid folder", http.MethodGet, "/api/email/threads?folder_id=nope", nil, http.StatusBadRequest},
		{"threads missing thread", http.MethodGet, "/api/email/threads/missing", nil, http.StatusBadRequest},
		{"messages actions method", http.MethodGet, "/api/email/messages/actions", nil, http.StatusMethodNotAllowed},
		{"messages actions malformed", http.MethodPost, "/api/email/messages/actions", `{`, http.StatusBadRequest},
		{"messages path short", http.MethodGet, "/api/email/messages/1", nil, http.StatusNotFound},
		{"messages invalid id", http.MethodGet, "/api/email/messages/nope/html", nil, http.StatusBadRequest},
		{"messages attachment short", http.MethodGet, "/api/email/messages/1/attachments", nil, http.StatusNotFound},
		{"messages unknown action path", http.MethodGet, "/api/email/messages/1/unknown", nil, http.StatusNotFound},
		{"message html method", http.MethodPost, "/api/email/messages/" + id + "/html", nil, http.StatusMethodNotAllowed},
		{"message attachment method", http.MethodPost, "/api/email/messages/" + id + "/attachments/1", nil, http.StatusMethodNotAllowed},
		{"draft collection method", http.MethodGet, "/api/email/drafts", nil, http.StatusMethodNotAllowed},
		{"draft collection malformed", http.MethodPost, "/api/email/drafts", `{`, http.StatusBadRequest},
		{"draft invalid id", http.MethodGet, "/api/email/drafts/nope", nil, http.StatusBadRequest},
		{"draft missing", http.MethodGet, "/api/email/drafts/999999", nil, http.StatusNotFound},
		{"draft malformed patch", http.MethodPatch, "/api/email/drafts/" + id, `{`, http.StatusBadRequest},
		{"draft method", http.MethodPut, "/api/email/drafts/" + id, nil, http.StatusMethodNotAllowed},
		{"draft send method", http.MethodGet, "/api/email/drafts/" + id + "/send", nil, http.StatusMethodNotAllowed},
		{"draft attachment no multipart", http.MethodPost, "/api/email/drafts/" + id + "/attachments", nil, http.StatusBadRequest},
		{"draft attachment method", http.MethodGet, "/api/email/drafts/" + id + "/attachments", nil, http.StatusMethodNotAllowed},
		{"draft unknown child", http.MethodGet, "/api/email/drafts/" + id + "/unknown", nil, http.StatusNotFound},
		{"preferences malformed", http.MethodPut, "/api/email/preferences", `{`, http.StatusBadRequest},
		{"preferences missing account", http.MethodPut, "/api/email/preferences", Preferences{DefaultAccount: "missing"}, http.StatusNotFound},
		{"preferences method", http.MethodPatch, "/api/email/preferences", nil, http.StatusMethodNotAllowed},
		{"recipients method", http.MethodPost, "/api/email/recipients", nil, http.StatusMethodNotAllowed},
		{"folder malformed", http.MethodPost, "/api/email/accounts/" + account.ID + "/folders", `{`, http.StatusBadRequest},
		{"folder invalid ID", http.MethodPatch, "/api/email/accounts/" + account.ID + "/folders/nope", map[string]string{"Name": "x"}, http.StatusBadRequest},
		{"folder method", http.MethodPut, "/api/email/accounts/" + account.ID + "/folders/1", nil, http.StatusMethodNotAllowed},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got := emailAPIRequest(t, a, "alice", tc.method, tc.target, tc.body)
			if got.Code != tc.want {
				t.Fatalf("status=%d want=%d body=%s", got.Code, tc.want, got.Body.String())
			}
		})
	}

	rec = emailAPIRequest(t, a, "alice", http.MethodGet, "/api/email/recipients?limit=0", nil)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"recipients"`) {
		t.Fatalf("recipients status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = emailAPIRequest(t, a, "alice", http.MethodGet, "/api/email/recipients?limit=2", nil)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "alice@example.test") ||
		!strings.Contains(rec.Body.String(), "bob@example.test") || strings.Contains(rec.Body.String(), "carol@example.test") {
		t.Fatalf("deduplicated recipients status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = emailAPIRequest(t, a, "alice", http.MethodDelete, "/api/email/drafts/"+id+"/attachments/missing", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("remove missing draft attachment status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = emailAPIRequest(t, a, "alice", http.MethodGet, "/api/email/mailboxes?account_id="+account.ID, nil)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"starred"`) {
		t.Fatalf("mailboxes status=%d body=%s", rec.Code, rec.Body.String())
	}
}

type failingEmailReader struct{}

func (failingEmailReader) Read([]byte) (int, error) { return 0, io.ErrUnexpectedEOF }

type failingEmailWriter struct{}

func (failingEmailWriter) Write([]byte) (int, error) { return 0, io.ErrClosedPipe }

func TestEmailMIMEAlternateBodyAttachmentDateAndWriterMatrix(t *testing.T) {
	dir := t.TempDir()
	write := func(name, raw string) parsedMail {
		t.Helper()
		path := filepath.Join(dir, name)
		if err := os.WriteFile(path, []byte(raw), 0o600); err != nil {
			t.Fatal(err)
		}
		parsed, err := parseMailFile(path)
		if err != nil {
			t.Fatal(err)
		}
		return parsed
	}
	broken := write("broken.eml", "")
	if broken.ParseError == "" || broken.ThreadKey == "" {
		t.Fatalf("broken message=%+v", broken)
	}
	htmlOnly := write("html.eml", "From: a@example.test\r\nTo: b@example.test\r\nSubject: HTML\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hello <b>HTML</b></p>")
	if htmlOnly.HTML == "" || !strings.Contains(htmlOnly.Text, "Hello") {
		t.Fatalf("HTML fallback=%+v", htmlOnly)
	}
	long := write("long.eml", "From: a@example.test\r\nTo: b@example.test\r\nSubject: Long\r\n\r\n"+strings.Repeat("word ", 100))
	if !strings.HasSuffix(long.Snippet, "…") {
		t.Fatalf("long snippet=%q", long.Snippet)
	}
	fallbackThread := write("fallback.eml", "From: a@example.test\r\nTo: b@example.test\r\nSubject: No IDs\r\n\r\nbody")
	if fallbackThread.ThreadKey != fallbackThread.Hash {
		t.Fatalf("fallback thread=%q hash=%q", fallbackThread.ThreadKey, fallbackThread.Hash)
	}

	if err := parseMIMEPart(textproto.MIMEHeader{}, strings.NewReader("body"), "multipart/mixed", nil, "1", &parsedMail{}); err == nil {
		t.Fatal("multipart without boundary accepted")
	}
	if err := parseMIMEPart(textproto.MIMEHeader{}, failingEmailReader{}, "text/plain", nil, "1", &parsedMail{}); err == nil {
		t.Fatal("body read error ignored")
	}
	message := strings.Join([]string{
		"--matrix",
		"Content-Type: text/plain; charset=iso-8859-1",
		"",
		"first",
		"--matrix",
		"",
		"second",
		"--matrix",
		"Content-Type: text/html",
		"",
		"<p>first</p>",
		"--matrix",
		"Content-Type: text/html",
		"",
		"<p>second</p>",
		"--matrix",
		"Content-Type: image/png",
		"Content-Disposition: inline",
		"Content-ID: <pixel>",
		"",
		"image",
		"--matrix--",
		"",
	}, "\r\n")
	parts := parsedMail{}
	if err := parseMIMEPart(textproto.MIMEHeader{}, strings.NewReader(message), "multipart/mixed", map[string]string{"boundary": "matrix"}, "1", &parts); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(parts.Text, "first\n\nsecond") || !strings.Contains(parts.HTML, "<hr>") ||
		len(parts.Attachments) != 1 || parts.Attachments[0].Filename != "attachment-1.5" || !parts.Attachments[0].Inline {
		t.Fatalf("multipart alternate parts=%+v", parts)
	}
	for _, value := range []string{
		time.Unix(1_800_000_000, 0).UTC().Format(time.RFC1123Z),
		time.Unix(1_800_000_000, 0).UTC().Format(time.RFC822Z),
		time.Unix(1_800_000_000, 0).UTC().Format(time.RFC3339),
	} {
		if parseMailDate(value) == 0 {
			t.Errorf("mail date rejected: %s", value)
		}
	}
	if parseMailDate("invalid") != 0 {
		t.Fatal("invalid mail date accepted")
	}
	var encoded bytes.Buffer
	if err := writeBase64Lines(&encoded, bytes.Repeat([]byte("x"), 100)); err != nil || !strings.Contains(encoded.String(), "\r\n") {
		t.Fatalf("base64 lines=%q err=%v", encoded.String(), err)
	}
	if err := writeBase64Lines(failingEmailWriter{}, []byte("x")); err == nil {
		t.Fatal("base64 writer error ignored")
	}
}

func TestEmailSMTPAuthAddressAndMessageConstructionBranchMatrix(t *testing.T) {
	authenticator := loginAuth{username: "alice", password: "secret"}
	if _, _, err := authenticator.Start(&smtp.ServerInfo{Name: "mail.example.test", TLS: false}); err == nil {
		t.Fatal("LOGIN accepted cleartext non-loopback server")
	}
	if mechanism, initial, err := authenticator.Start(&smtp.ServerInfo{Name: "mail.example.test", TLS: true}); err != nil ||
		mechanism != "LOGIN" || string(initial) != "alice" {
		t.Fatalf("LOGIN start mechanism=%q initial=%q err=%v", mechanism, initial, err)
	}
	for _, tc := range []struct {
		challenge string
		more      bool
		want      string
		wantErr   bool
	}{
		{"ignored", false, "", false},
		{base64.StdEncoding.EncodeToString([]byte("Username:")), true, "alice", false},
		{"Password:", true, "secret", false},
		{"OTP:", true, "", true},
	} {
		got, err := authenticator.Next([]byte(tc.challenge), tc.more)
		if string(got) != tc.want || (err != nil) != tc.wantErr {
			t.Errorf("LOGIN challenge %q more=%v got=%q err=%v", tc.challenge, tc.more, got, err)
		}
	}
	config := (&App{}).smtpTLS("mail.example.test")
	if config.ServerName != "mail.example.test" || config.MinVersion != tls.VersionTLS12 {
		t.Fatalf("default SMTP TLS config=%+v", config)
	}
	if err := (&App{}).testSMTP(context.Background(), Account{SMTPHost: "mail.example.test", SMTPSecurity: securityNone}, "secret"); err == nil {
		t.Fatal("plaintext remote SMTP accepted")
	}
	for _, raw := range []string{"broken <", "local-address"} {
		if _, _, err := formatAddressList(raw); err == nil {
			t.Errorf("invalid address list accepted: %q", raw)
		}
	}
	env, header, err := formatAddressList(`Renée <renee@example.test>`)
	if err != nil || len(env) != 1 || !strings.Contains(header, "=?utf-8?") {
		t.Fatalf("encoded address env=%v header=%q err=%v", env, header, err)
	}
	for _, message := range []outgoingMessage{
		{To: "bob@example.test", Subject: "bad\nsubject"},
		{To: "broken <"},
		{To: "bob@example.test", Cc: "broken <"},
		{To: "bob@example.test", Bcc: "broken <"},
		{Draft: false},
	} {
		if _, _, err := buildMessage(message); err == nil {
			t.Errorf("invalid outgoing message accepted: %+v", message)
		}
	}
	raw, recipients, err := buildMessage(outgoingMessage{
		FromName: "Alice", FromEmail: "alice@example.test", To: "bob@example.test", Subject: "Inline",
		Body: "body", IncludeBcc: true, Bcc: "hidden@example.test",
		Attachments: []outgoingAttachment{
			{Filename: "../pixel.bin", Data: []byte("one"), Inline: true, ContentID: "<pixel>"},
			{Filename: "plain.bin", MIMEType: "", Data: []byte("two")},
		},
	})
	if err != nil || len(recipients) != 2 || !bytes.Contains(raw, []byte("Content-Disposition: inline")) ||
		!strings.Contains(strings.ToLower(string(raw)), "content-id: <pixel>") || !bytes.Contains(raw, []byte("application/octet-stream")) ||
		bytes.Contains(raw, []byte("../pixel.bin")) {
		t.Fatalf("inline message recipients=%v err=%v raw=%s", recipients, err, raw)
	}
}
