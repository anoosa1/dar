// Email daemon — mbsync for IMAP sync, Maildir++ storage, net/smtp for sending.
package main

import (
	"crypto/tls"
	"encoding/json"
	"flag"
	"io"
	"fmt"
	"log"
	"net/http"
	"net/mail"
	"net/smtp"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
)

var (
	port      = flag.Int("port", 8088, "HTTP port")
	dataRoot  = flag.String("data-root", "data", "Root directory for user data")
	mbsyncBin = flag.String("mbsync", "mbsync", "Path to mbsync binary")
	syncMins  = flag.Int("sync-interval", 5, "Auto-sync interval in minutes")
)

var mu sync.Mutex
var syncStatus = map[string]bool{}

func main() {
	flag.Parse()
	os.MkdirAll(*dataRoot, 0755)
	mux := http.NewServeMux()
	mux.HandleFunc("/accounts", handleAccounts)
	mux.HandleFunc("/sync", handleSync)
	mux.HandleFunc("/folders", handleFolders)
	mux.HandleFunc("/messages", handleMessages)
	mux.HandleFunc("/message", handleMessage)
	mux.HandleFunc("/send", handleSend)
	mux.HandleFunc("/health", handleHealth)
	go pollLoop()
	addr := fmt.Sprintf(":%d", *port)
	log.Printf("[email] listening on :%d", *port)
	log.Fatal(http.ListenAndServe(addr, mux))
}

// ── Types ──

type Account struct {
	ID       string `json:"id"`
	Email    string `json:"email"`
	Name     string `json:"name"`
	ImapHost string `json:"imap_host"`
	ImapPort int    `json:"imap_port"`
	ImapTLS  bool   `json:"imap_tls"`
	SmtpHost string `json:"smtp_host"`
	SmtpPort int    `json:"smtp_port"`
	SmtpTLS  bool   `json:"smtp_tls"`
	Username string `json:"username"`
}

type MsgMeta struct {
	ID      string `json:"id"`
	Folder  string `json:"folder"`
	Account string `json:"account"`
	From    string `json:"from"`
	To      string `json:"to"`
	Cc      string `json:"cc"`
	Subject string `json:"subject"`
	Date    string `json:"date"`
	Size    int64  `json:"size"`
	Seen    bool   `json:"seen"`
	Flagged bool   `json:"flagged"`
}

type Folder struct {
	Name  string `json:"name"`
	Count int    `json:"count"`
	New   int    `json:"new"`
}

// ── Helpers ──

func userID(r *http.Request) string {
	if uid := r.Header.Get("X-User-Id"); uid != "" { return uid }
	if uid := r.URL.Query().Get("user_id"); uid != "" { return uid }
	return "default"
}

func emailRoot(uid string) string {
	d := filepath.Join(*dataRoot, uid, "email")
	os.MkdirAll(d, 0755)
	return d
}

func accountsPath(uid string) string  { return filepath.Join(emailRoot(uid), "accounts.json") }
func accountDir(uid, aid string) string { return filepath.Join(emailRoot(uid), "accounts", aid) }
func maildirRoot(uid, aid string) string { return filepath.Join(accountDir(uid, aid), "mail") }

func loadAccounts(uid string) []Account {
	data, _ := os.ReadFile(accountsPath(uid))
	var a []Account
	json.Unmarshal(data, &a)
	if a == nil { a = []Account{} }
	return a
}
func saveAccounts(uid string, a []Account) error {
	data, _ := json.Marshal(a)
	return os.WriteFile(accountsPath(uid), data, 0644)
}

// ── Account CRUD ──

func handleAccounts(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	switch r.Method {
	case "GET":
		writeJSON(w, map[string]any{"accounts": loadAccounts(uid)})
	case "POST":
		var a Account
		json.NewDecoder(r.Body).Decode(&a)
		a.ID = fmt.Sprintf("%d", time.Now().UnixNano())
		if a.ImapPort == 0 { a.ImapPort = 993 }
		if a.SmtpPort == 0 { a.SmtpPort = 587 }
		accts := loadAccounts(uid)
		accts = append(accts, a)
		saveAccounts(uid, accts)
		writeMbsyncConfig(uid, a)
		writeJSON(w, a)
	case "PUT":
		var a Account
		json.NewDecoder(r.Body).Decode(&a)
		accts := loadAccounts(uid)
		for i, e := range accts {
			if e.ID == a.ID { accts[i] = a; saveAccounts(uid, accts); writeMbsyncConfig(uid, a); writeJSON(w, a); return }
		}
		http.Error(w, "not found", 404)
	case "DELETE":
		id := r.URL.Query().Get("id")
		accts := loadAccounts(uid)
		for i, a := range accts {
			if a.ID == id {
				accts = append(accts[:i], accts[i+1:]...)
				saveAccounts(uid, accts)
				os.RemoveAll(accountDir(uid, id))
				// Remove mbsync config section
				removeMbsyncConfig(uid, id)
				writeJSON(w, map[string]string{"status": "ok"})
				return
			}
		}
		http.Error(w, "not found", 404)
	default:
		http.Error(w, "method not allowed", 405)
	}
}

// ── mbsync config ──

func mbsyncConfigPath(uid string) string {
	return filepath.Join(emailRoot(uid), "mbsyncrc")
}

func writeMbsyncConfig(uid string, a Account) {
	cfgPath := mbsyncConfigPath(uid)
	existing, _ := os.ReadFile(cfgPath)
	// Remove old section for this account if present
	newCfg := removeAccountSection(string(existing), a.ID)
	ssl := "IMAPS"
	if !a.ImapTLS { ssl = "None" }
	port := a.ImapPort
	if port == 0 { port = 993 }
	root := maildirRoot(uid, a.ID)
	entry := fmt.Sprintf(`
IMAPAccount %s
Host %s
Port %d
User %s
PassCmd "pass dar/email/%s"
SSLType %s
CertificateFile /etc/ssl/certs/ca-certificates.crt

IMAPStore %s-remote
Account %s

MaildirStore %s-local
Path %s/
Inbox %s/INBOX
SubFolders Verbatim

Channel %s
Master :%s-remote:
Slave :%s-local:
Patterns *
Create Slave
SyncState *
`, a.ID, a.ImapHost, port, a.Username, a.ID, ssl,
		a.ID, a.ID,
		a.ID, root, root,
		a.ID, a.ID, a.ID)
	newCfg += entry
	os.WriteFile(cfgPath, []byte(newCfg), 0644)
}

func removeMbsyncConfig(uid, id string) {
	cfgPath := mbsyncConfigPath(uid)
	existing, _ := os.ReadFile(cfgPath)
	newCfg := removeAccountSection(string(existing), id)
	os.WriteFile(cfgPath, []byte(newCfg), 0644)
}

func removeAccountSection(cfg, id string) string {
	lines := strings.Split(cfg, "\n")
	var out []string
	skip := false
	for _, line := range lines {
		trimmed := strings.TrimSpace(line)
		// Start of section for this account
		if strings.HasPrefix(trimmed, "IMAPAccount "+id) || strings.Contains(trimmed, " "+id+"-") {
			skip = true
		}
		if skip {
			if trimmed == "" { skip = false }
			continue
		}
		out = append(out, line)
	}
	return strings.Join(out, "\n")
}

// ── Sync ──

func handleSync(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := userID(r)
	accountID := r.URL.Query().Get("account")
	key := uid + "/" + accountID
	mu.Lock()
	if syncStatus[key] {
		mu.Unlock()
		writeJSON(w, map[string]string{"status": "already syncing"})
		return
	}
	syncStatus[key] = true
	mu.Unlock()
	go func() {
		defer func() { mu.Lock(); delete(syncStatus, key); mu.Unlock() }()
		cfgPath := mbsyncConfigPath(uid)
		cmd := exec.Command(*mbsyncBin, "-c", cfgPath, accountID)
		cmd.Stdout = os.Stdout
		cmd.Stderr = os.Stderr
		log.Printf("[email] syncing account %s", accountID)
		if err := cmd.Run(); err != nil {
			log.Printf("[email] sync %s failed: %v", accountID, err)
		} else {
			log.Printf("[email] sync %s complete", accountID)
		}
	}()
	writeJSON(w, map[string]string{"status": "syncing"})
}

// ── Maildir++ API ──

func handleFolders(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	accountID := r.URL.Query().Get("account")
	root := maildirRoot(uid, accountID)
	var folders []Folder
	filepath.Walk(root, func(path string, info os.FileInfo, err error) error {
		if err != nil || path == root { return nil }
		if info.IsDir() && (info.Name() == "cur" || info.Name() == "new" || info.Name() == "tmp") {
			// Get parent folder name
			parent := filepath.Dir(path)
			if parent == root { return nil }
			name := strings.TrimPrefix(parent, root+"/")
			name = strings.ReplaceAll(name, "/", ".")
			if strings.HasPrefix(name, ".") { name = name[1:] }
			if name == "INBOX" { name = "" } // INBOX is empty string convention
			curEntries, _ := os.ReadDir(filepath.Join(parent, "cur"))
			newEntries, _ := os.ReadDir(filepath.Join(parent, "new"))
			folders = append(folders, Folder{
				Name:  name,
				Count: len(curEntries),
				New:   len(newEntries),
			})
			return filepath.SkipDir
		}
		return nil
	})
	sort.Slice(folders, func(i, j int) bool {
		ni, nj := folders[i].Name, folders[j].Name
		// INBOX first, then alphabetical
		if ni == "" { return true }
		if nj == "" { return false }
		if strings.HasPrefix(ni, "Sent") { return true }
		if strings.HasPrefix(nj, "Sent") && !strings.HasPrefix(ni, "Sent") { return false }
		return ni < nj
	})
	if folders == nil { folders = []Folder{} }
	writeJSON(w, map[string]any{"folders": folders})
}

func handleMessages(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	accountID := r.URL.Query().Get("account")
	folderName := r.URL.Query().Get("folder")
	pageStr := r.URL.Query().Get("page")
	page, _ := strconv.Atoi(pageStr)
	if page < 1 { page = 1 }
	perPage := 50

	root := maildirRoot(uid, accountID)
	folderPath := root
	if folderName != "" {
		// Convert folder name to Maildir++ path
		// INBOX is root, subfolders are .name
		folderPath = filepath.Join(root, "."+folderName)
	}

	var metas []MsgMeta
	// Read from cur/ (seen) and new/ (unseen)
	for _, sub := range []string{"new", "cur"} {
		dir := filepath.Join(folderPath, sub)
		entries, _ := os.ReadDir(dir)
		for _, e := range entries {
			if e.IsDir() { continue }
			msgPath := filepath.Join(dir, e.Name())
			info, _ := e.Info()
			meta := parseMsgHeaders(msgPath, accountID, folderName, sub == "cur")
			meta.Size = info.Size()
			metas = append(metas, meta)
		}
	}
	sort.Slice(metas, func(i, j int) bool { return metas[i].Date > metas[j].Date })
	start := (page - 1) * perPage
	if start >= len(metas) {
		writeJSON(w, map[string]any{"messages": []MsgMeta{}, "page": page, "total": len(metas)})
		return
	}
	end := start + perPage
	if end > len(metas) { end = len(metas) }
	writeJSON(w, map[string]any{"messages": metas[start:end], "page": page, "total": len(metas)})
}

func parseMsgHeaders(path, accountID, folder string, seen bool) MsgMeta {
	f, err := os.Open(path)
	if err != nil {
		return MsgMeta{ID: filepath.Base(path), Folder: folder, Account: accountID}
	}
	defer f.Close()
	msg, err := mail.ReadMessage(f)
	if err != nil {
		return MsgMeta{ID: filepath.Base(path), Folder: folder, Account: accountID}
	}
	from := msg.Header.Get("From")
	to := msg.Header.Get("To")
	cc := msg.Header.Get("Cc")
	subject := msg.Header.Get("Subject")
	date := msg.Header.Get("Date")
	return MsgMeta{
		ID:      filepath.Base(path),
		Folder:  folder,
		Account: accountID,
		From:    from,
		To:      to,
		Cc:      cc,
		Subject: subject,
		Date:    date,
		Seen:    seen,
	}
}

func handleMessage(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	id := r.URL.Query().Get("id")
	accountID := r.URL.Query().Get("account")
	folder := r.URL.Query().Get("folder")
	root := maildirRoot(uid, accountID)

	if r.Method == "POST" {
		// Flag actions
		action := r.URL.Query().Get("action")
		folderPath := root
		if folder != "" { folderPath = filepath.Join(root, "."+folder) }
		switch action {
		case "read":
			moveMsg(folderPath, id, "new", "cur")
		case "unread":
			moveMsg(folderPath, id, "cur", "new")
		case "move":
			dst := r.URL.Query().Get("to")
			if dst == "" { http.Error(w, "missing to", 400); return }
			// Move between folders
			srcFolder := folderPath
			dstFolder := root
			if dst != "" { dstFolder = filepath.Join(root, "."+dst) }
			os.MkdirAll(filepath.Join(dstFolder, "cur"), 0755)
			os.MkdirAll(filepath.Join(dstFolder, "new"), 0755)
			moveMsg(srcFolder, id, "", filepath.Join(dstFolder, "cur"))
		case "flag":
			// Toggle flagged — store in file tags or rename
			// Simple: just acknowledge
		default:
			http.Error(w, "unknown action", 400)
			return
		}
		writeJSON(w, map[string]string{"status": "ok"})
		return
	}

	// GET: return full message content
	searchDirs := []string{
		filepath.Join(root, "cur", id),
		filepath.Join(root, "new", id),
	}
	if folder != "" {
		fp := filepath.Join(root, "."+folder)
		searchDirs = []string{filepath.Join(fp, "cur", id), filepath.Join(fp, "new", id)}
	}
	var foundPath string
	for _, p := range searchDirs {
		if _, err := os.Stat(p); err == nil { foundPath = p; break }
	}
	if foundPath == "" { http.Error(w, "not found", 404); return }

	data, _ := os.ReadFile(foundPath)
	msg, err := mail.ReadMessage(strings.NewReader(string(data)))
	if err != nil { http.Error(w, "parse error", 500); return }
	body, _ := io.ReadAll(msg.Body)
	result := map[string]any{
		"id":      id,
		"from":    msg.Header.Get("From"),
		"to":      msg.Header.Get("To"),
		"cc":      msg.Header.Get("Cc"),
		"subject": msg.Header.Get("Subject"),
		"date":    msg.Header.Get("Date"),
		"body":    string(body),
	}
	writeJSON(w, result)
}

func moveMsg(folderPath, id, fromSub, toPath string) {
	if fromSub != "" {
		src := filepath.Join(folderPath, fromSub, id)
		dst := filepath.Join(folderPath, toPath, id)
		if strings.HasSuffix(toPath, "/cur") || strings.HasSuffix(toPath, "/new") {
			dst = filepath.Join(toPath, id)
		} else {
			dst = filepath.Join(toPath, id)
		}
		os.Rename(src, dst)
	} else {
		// Search for the message in cur/ or new/
		for _, sub := range []string{"cur", "new"} {
			src := filepath.Join(folderPath, sub, id)
			if _, err := os.Stat(src); err == nil {
				os.MkdirAll(filepath.Dir(toPath), 0755)
				os.Rename(src, toPath)
				return
			}
		}
	}
}

// ── SMTP Send ──

func handleSend(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", 405)
		return
	}
	uid := userID(r)
	var req struct {
		AccountID string `json:"account_id"`
		To        string `json:"to"`
		Cc        string `json:"cc"`
		Bcc       string `json:"bcc"`
		Subject   string `json:"subject"`
		Body      string `json:"body"`
	}
	json.NewDecoder(r.Body).Decode(&req)

	// Find account
	var acct *Account
	for _, a := range loadAccounts(uid) {
		if a.ID == req.AccountID { acct = &a; break }
	}
	if acct == nil { http.Error(w, "account not found", 404); return }

	// Build email
	msg := fmt.Sprintf("From: %s\r\nTo: %s\r\n", acct.Email, req.To)
	if req.Cc != "" { msg += "Cc: " + req.Cc + "\r\n" }
	msg += "Subject: " + req.Subject + "\r\n"
	msg += "MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n"
	msg += req.Body

	// Send via SMTP
	addr := fmt.Sprintf("%s:%d", acct.SmtpHost, acct.SmtpPort)
	var auth smtp.Auth
	if acct.SmtpTLS {
		auth = smtp.PlainAuth("", acct.Username, "", acct.SmtpHost) // password loaded externally
	}

	recipients := strings.Split(req.To, ",")
	if req.Cc != "" { recipients = append(recipients, strings.Split(req.Cc, ",")...) }
	if req.Bcc != "" { recipients = append(recipients, strings.Split(req.Bcc, ",")...) }

	// Try TLS first, fallback to plain
	var err error
	if acct.SmtpTLS {
		tlsCfg := &tls.Config{ServerName: acct.SmtpHost}
		conn, e := tls.Dial("tcp", addr, tlsCfg)
		if e != nil { err = e } else {
			defer conn.Close()
			client, e := smtp.NewClient(conn, acct.SmtpHost)
			if e != nil { err = e } else {
				defer client.Quit()
				if auth != nil { client.Auth(auth) }
				client.Mail(acct.Email)
				for _, rcpt := range recipients {
					client.Rcpt(strings.TrimSpace(rcpt))
				}
				wc, e := client.Data()
				if e != nil { err = e } else {
					wc.Write([]byte(msg))
					wc.Close()
				}
			}
		}
	} else {
		err = smtp.SendMail(addr, auth, acct.Email, recipients, []byte(msg))
	}
	if err != nil {
		log.Printf("[email] send failed: %v", err)
		http.Error(w, fmt.Sprintf("send failed: %v", err), 500)
		return
	}

	// Save copy to Sent folder
	sentDir := filepath.Join(maildirRoot(uid, req.AccountID), ".Sent", "cur")
	os.MkdirAll(sentDir, 0755)
	sentFile := filepath.Join(sentDir, fmt.Sprintf("%d.mail", time.Now().UnixNano()))
	os.WriteFile(sentFile, []byte(msg), 0644)

	writeJSON(w, map[string]string{"status": "sent"})
}

// ── Polling ──

func pollLoop() {
	interval := time.Duration(*syncMins) * time.Minute
	for {
		time.Sleep(interval)
		// Discover all users
		entries, _ := os.ReadDir(*dataRoot)
		for _, e := range entries {
			if !e.IsDir() { continue }
			uid := e.Name()
			for _, a := range loadAccounts(uid) {
				key := uid + "/" + a.ID
				mu.Lock()
				if syncStatus[key] { mu.Unlock(); continue }
				syncStatus[key] = true
				mu.Unlock()
				go func(uid, aid string) {
					defer func() { mu.Lock(); delete(syncStatus, uid+"/"+aid); mu.Unlock() }()
					cfgPath := mbsyncConfigPath(uid)
					cmd := exec.Command(*mbsyncBin, "-c", cfgPath, aid)
					cmd.Stdout = os.Stdout
					cmd.Stderr = os.Stderr
					log.Printf("[email] auto-sync %s", aid)
					if err := cmd.Run(); err != nil {
						log.Printf("[email] auto-sync %s failed: %v", aid, err)
					}
				}(uid, a.ID)
			}
		}
	}
}

// ── Health ──

func handleHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]string{"status": "ok"})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}
