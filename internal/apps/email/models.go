package email

import "time"

type securityMode string

const (
	securityTLS      securityMode = "tls"
	securitySTARTTLS securityMode = "starttls"
	securityNone     securityMode = "none"
)

type Account struct {
	ID               string       `json:"id"`
	Email            string       `json:"email"`
	Name             string       `json:"name"`
	Username         string       `json:"username"`
	IMAPHost         string       `json:"imap_host"`
	IMAPPort         int          `json:"imap_port"`
	IMAPSecurity     securityMode `json:"imap_security"`
	SMTPHost         string       `json:"smtp_host"`
	SMTPPort         int          `json:"smtp_port"`
	SMTPSecurity     securityMode `json:"smtp_security"`
	Enabled          bool         `json:"enabled"`
	SyncMinutes      int          `json:"sync_minutes"`
	MaxMessages      int          `json:"max_messages"`
	SaveSent         bool         `json:"save_sent"`
	LastSyncAt       int64        `json:"last_sync_at,omitempty"`
	LastSyncError    string       `json:"last_sync_error,omitempty"`
	Password         string       `json:"password,omitempty"`
	PasswordKeyID    string       `json:"-"`
	PasswordNonce    []byte       `json:"-"`
	PasswordCipher   []byte       `json:"-"`
	mailRootOverride string
}

type Folder struct {
	ID        int64  `json:"id"`
	AccountID string `json:"account_id"`
	Name      string `json:"name"`
	Role      string `json:"role"`
	Total     int    `json:"total"`
	Unread    int    `json:"unread"`
}

type ThreadSummary struct {
	ID             string `json:"id"`
	AccountID      string `json:"account_id"`
	AccountEmail   string `json:"account_email"`
	Subject        string `json:"subject"`
	Participants   string `json:"participants"`
	Snippet        string `json:"snippet"`
	LatestAt       int64  `json:"latest_at"`
	MessageCount   int    `json:"message_count"`
	UnreadCount    int    `json:"unread_count"`
	Flagged        bool   `json:"flagged"`
	HasAttachments bool   `json:"has_attachments"`
}

type Message struct {
	ID              int64        `json:"id"`
	AccountID       string       `json:"account_id"`
	FolderID        int64        `json:"folder_id"`
	FolderRole      string       `json:"folder_role"`
	ThreadKey       string       `json:"thread_key"`
	HeaderMessageID string       `json:"message_id"`
	From            string       `json:"from"`
	To              string       `json:"to"`
	Cc              string       `json:"cc"`
	Bcc             string       `json:"bcc,omitempty"`
	Subject         string       `json:"subject"`
	Date            int64        `json:"date"`
	Text            string       `json:"text,omitempty"`
	HasHTML         bool         `json:"has_html"`
	Read            bool         `json:"read"`
	Flagged         bool         `json:"flagged"`
	Answered        bool         `json:"answered"`
	Draft           bool         `json:"draft"`
	Attachments     []Attachment `json:"attachments"`
}

type Attachment struct {
	Part      string `json:"part"`
	Filename  string `json:"filename"`
	MIMEType  string `json:"mime_type"`
	Size      int64  `json:"size"`
	ContentID string `json:"content_id,omitempty"`
	Inline    bool   `json:"inline"`
}

type DraftInput struct {
	AccountID  string `json:"account_id"`
	To         string `json:"to"`
	Cc         string `json:"cc"`
	Bcc        string `json:"bcc"`
	Subject    string `json:"subject"`
	Body       string `json:"body"`
	InReplyTo  string `json:"in_reply_to,omitempty"`
	References string `json:"references,omitempty"`
}

type SyncStatus struct {
	AccountID string `json:"account_id"`
	State     string `json:"state"`
	Trigger   string `json:"trigger"`
	StartedAt int64  `json:"started_at,omitempty"`
	EndedAt   int64  `json:"ended_at,omitempty"`
	Scanned   int    `json:"scanned,omitempty"`
	Updated   int    `json:"updated,omitempty"`
	Removed   int    `json:"removed,omitempty"`
	Error     string `json:"error,omitempty"`
}

type Preferences struct {
	DefaultAccount string `json:"default_account"`
	Density        string `json:"density"`
	ReadingPane    string `json:"reading_pane"`
}

func defaultPreferences() Preferences {
	return Preferences{Density: "comfortable", ReadingPane: "vertical"}
}

func nowUnix() int64 { return time.Now().Unix() }
