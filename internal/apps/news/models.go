package news

import "time"

const compatibilityVersion = "28.6.0"

type Folder struct {
	ID   int64  `json:"id"`
	Name string `json:"name"`
}

type Feed struct {
	ID                int64  `json:"id"`
	URL               string `json:"url"`
	Title             string `json:"title"`
	FaviconLink       string `json:"faviconLink"`
	Added             int64  `json:"added"`
	NextUpdateTime    int64  `json:"nextUpdateTime"`
	FolderID          int64  `json:"folderId"`
	UnreadCount       int64  `json:"unreadCount"`
	Ordering          int    `json:"ordering"`
	Link              string `json:"link"`
	Pinned            bool   `json:"pinned"`
	UpdateErrorCount  int    `json:"updateErrorCount"`
	LastUpdateError   string `json:"lastUpdateError"`
	LastUpdateTime    int64  `json:"lastUpdateTime,omitempty"`
	ArticlesPerUpdate int    `json:"articlesPerUpdate,omitempty"`
	UpdateEnabled     bool   `json:"updateEnabled,omitempty"`
	UpdateMode        int    `json:"updateMode,omitempty"`
	FullTextEnabled   bool   `json:"fullTextEnabled,omitempty"`
	KeywordFilters    string `json:"keywordFilters,omitempty"`
	BasicAuthUser     string `json:"basicAuthUser,omitempty"`
	BasicAuthPassword string `json:"-"`
	credentialKeyID   string
	credentialNonce   []byte
	credentialCipher  []byte
}

type Item struct {
	ID               int64   `json:"id"`
	GUID             string  `json:"guid"`
	GUIDHash         string  `json:"guidHash"`
	URL              string  `json:"url"`
	Title            string  `json:"title"`
	Author           *string `json:"author"`
	PubDate          int64   `json:"pubDate"`
	Body             string  `json:"body"`
	EnclosureMime    *string `json:"enclosureMime"`
	EnclosureLink    *string `json:"enclosureLink"`
	MediaThumbnail   *string `json:"mediaThumbnail"`
	MediaDescription *string `json:"mediaDescription"`
	FeedID           int64   `json:"feedId"`
	Unread           bool    `json:"unread"`
	Starred          bool    `json:"starred"`
	RTL              bool    `json:"rtl"`
	LastModified     int64   `json:"lastModified"`
	Fingerprint      string  `json:"fingerprint"`
	ViewedAt         int64   `json:"viewedAt,omitempty"`
}

type Preferences struct {
	DisableScrollRead  bool   `json:"disableScrollRead"`
	ShowAll            bool   `json:"showAll"`
	OldestFirst        bool   `json:"oldestFirst"`
	DisableAutoRefresh bool   `json:"disableAutoRefresh"`
	Thumbnails         string `json:"thumbnails"`
	Enclosures         string `json:"enclosures"`
	Images             string `json:"images"`
	Iframes            string `json:"iframes"`
	DisplayMode        string `json:"displayMode"`
	SplitMode          string `json:"splitMode"`
}

func defaultPreferences() Preferences {
	return Preferences{Thumbnails: "always", Enclosures: "always", Images: "ask", Iframes: "ask", DisplayMode: "default", SplitMode: "vertical"}
}

type refreshState struct {
	Running   bool      `json:"running"`
	StartedAt time.Time `json:"startedAt,omitempty"`
}
