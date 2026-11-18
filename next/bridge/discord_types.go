package main

import (
	"encoding/json"
	"strconv"
	"strings"
)

// Discord API objects, only the fields Dar uses.

// flexID is a snowflake or number Discord may send as either a string or a JSON number.
type flexID string

func (f *flexID) UnmarshalJSON(raw []byte) error {
	s := strings.Trim(string(raw), `"`)
	if s == "null" {
		s = ""
	}
	*f = flexID(s)
	return nil
}

// perm is a permission bit set, sent as a decimal string.
type perm int64

func (p *perm) UnmarshalJSON(raw []byte) error {
	n, err := strconv.ParseInt(strings.Trim(string(raw), `"`), 10, 64)
	if err == nil {
		*p = perm(n)
	}
	return nil
}

type dcUser struct {
	ID         string  `json:"id"`
	Username   string  `json:"username"`
	GlobalName *string `json:"global_name"`
	Bot        bool    `json:"bot"`
}

func (u dcUser) name() string {
	if u.GlobalName != nil && strings.TrimSpace(*u.GlobalName) != "" {
		return *u.GlobalName
	}
	return u.Username
}

type dcOverwrite struct {
	ID    string `json:"id"`
	Type  int    `json:"type"` // 0 role, 1 member
	Allow perm   `json:"allow"`
	Deny  perm   `json:"deny"`
}

type dcChannel struct {
	ID            string        `json:"id"`
	Type          int           `json:"type"`
	GuildID       string        `json:"guild_id"`
	Name          string        `json:"name"`
	Topic         *string       `json:"topic"`
	Position      int           `json:"position"`
	ParentID      *string       `json:"parent_id"`
	LastMessageID flexID        `json:"last_message_id"`
	Overwrites    []dcOverwrite `json:"permission_overwrites"`
	RecipientIDs  []string      `json:"recipient_ids"`
	Recipients    []dcUser      `json:"recipients"`
}

// Channel types Dar shows.
const (
	channelText     = 0
	channelDM       = 1
	channelVoice    = 2
	channelGroupDM  = 3
	channelCategory = 4
	channelNews     = 5
)

type dcRole struct {
	ID          string `json:"id"`
	Name        string `json:"name"`
	Permissions perm   `json:"permissions"`
}

type dcMember struct {
	UserID string   `json:"user_id"`
	User   *dcUser  `json:"user"`
	Roles  []string `json:"roles"`
	Nick   *string  `json:"nick"`
}

func (m dcMember) id() string {
	if m.User != nil {
		return m.User.ID
	}
	return m.UserID
}

// dcGuild is a guild in READY or GUILD_CREATE; without the CLIENT_STATE_V2 capability its
// properties are inline, with it they are under "properties". Both are read.
type dcGuild struct {
	ID          string      `json:"id"`
	Name        string      `json:"name"`
	OwnerID     string      `json:"owner_id"`
	Unavailable bool        `json:"unavailable"`
	Roles       []dcRole    `json:"roles"`
	Channels    []dcChannel `json:"channels"`
	Members     []dcMember  `json:"members"`
	Properties  *struct {
		Name    string `json:"name"`
		OwnerID string `json:"owner_id"`
	} `json:"properties"`
}

type dcAttachment struct {
	ID          string  `json:"id"`
	Filename    string  `json:"filename"`
	ContentType string  `json:"content_type"`
	Size        uint64  `json:"size"`
	URL         string  `json:"url"`
	Width       uint32  `json:"width"`
	Height      uint32  `json:"height"`
	Duration    float64 `json:"duration_secs"`
}

type dcEmoji struct {
	ID   *string `json:"id"`
	Name string  `json:"name"`
}

// key is how Dar names an emoji: the character itself, or "name:id" for a custom one.
func (e dcEmoji) key() string {
	if e.ID != nil && *e.ID != "" {
		return e.Name + ":" + *e.ID
	}
	return e.Name
}

type dcReaction struct {
	Emoji dcEmoji `json:"emoji"`
	Count int     `json:"count"`
	Me    bool    `json:"me"`
}

type dcMessage struct {
	ID                string         `json:"id"`
	ChannelID         string         `json:"channel_id"`
	GuildID           string         `json:"guild_id"`
	Author            dcUser         `json:"author"`
	Member            *dcMember      `json:"member"`
	Content           string         `json:"content"`
	Timestamp         string         `json:"timestamp"`
	EditedTimestamp   *string        `json:"edited_timestamp"`
	Type              int            `json:"type"`
	Flags             int            `json:"flags"`
	Attachments       []dcAttachment `json:"attachments"`
	Embeds            []dcEmbed      `json:"embeds"`
	Stickers          []dcSticker    `json:"sticker_items"`
	Mentions          []dcUser       `json:"mentions"`
	MentionRoles      []string       `json:"mention_roles"`
	MentionEveryone   bool           `json:"mention_everyone"`
	ReferencedMessage *dcMessage     `json:"referenced_message"`
	MessageReference  *struct {
		MessageID string `json:"message_id"`
	} `json:"message_reference"`
	Nonce     flexID       `json:"nonce"`
	Reactions []dcReaction `json:"reactions"`
}

type dcEmbed struct {
	Title       string `json:"title"`
	Description string `json:"description"`
	URL         string `json:"url"`
}

type dcSticker struct {
	ID         string `json:"id"`
	Name       string `json:"name"`
	FormatType int    `json:"format_type"`
}

// What the bridge hands the runtime for Discord.

type dcServerOut struct {
	ID         string `json:"id"`
	Name       string `json:"name"`
	Order      string `json:"order"`
	MutedUntil int64  `json:"mutedUntil"`
}

type dcChatOut struct {
	ID         string `json:"id"`
	Kind       string `json:"kind"` // direct, group, channel, voice
	Name       string `json:"name"`
	ServerID   string `json:"serverId,omitempty"`
	Section    string `json:"section,omitempty"`
	Order      string `json:"order,omitempty"`
	Topic      string `json:"topic,omitempty"`
	Unread     *int   `json:"unread,omitempty"`
	Mentions   *int   `json:"mentions,omitempty"`
	MutedUntil *int64 `json:"mutedUntil,omitempty"`
	At         int64  `json:"at,omitempty"`
}

type dcReactionOut struct {
	Emoji string `json:"emoji"`
	Count int    `json:"count"`
	Me    bool   `json:"me"`
}

type dcMessageOut struct {
	chatMessage
	Nonce     string          `json:"nonce,omitempty"`
	Mention   bool            `json:"mention,omitempty"`
	Reactions []dcReactionOut `json:"reactions,omitempty"`
}

func rawField(raw json.RawMessage, name string) (json.RawMessage, bool) {
	var fields map[string]json.RawMessage
	if json.Unmarshal(raw, &fields) != nil {
		return nil, false
	}
	value, ok := fields[name]
	return value, ok
}
