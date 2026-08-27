package codex

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/anoosa/dar/internal/apps"
	searchindex "github.com/anoosa/dar/internal/search"
)

func (a *App) IndexSearch(ctx context.Context, uid string, sink apps.SearchDocumentSink) error {
	return a.service.indexSearch(ctx, uid, sink)
}

func (s *Service) indexSearch(ctx context.Context, uid string, sink apps.SearchDocumentSink) error {
	active, err := s.store.listSessions(ctx, uid, false)
	if err != nil {
		return err
	}
	archived, err := s.store.listSessions(ctx, uid, true)
	if err != nil {
		return err
	}
	sessions := append(active, archived...)
	if len(sessions) == 0 {
		return nil
	}
	client, err := s.client(ctx, uid)
	if err != nil {
		return err
	}
	for _, session := range sessions {
		var response any
		if err := client.call(ctx, "thread/read", map[string]any{"threadId": session.ThreadID, "includeTurns": true}, &response); err != nil {
			if err := s.resumeThread(ctx, uid, client, session, &response); err != nil {
				return err
			}
		}
		messages := collectCodexMessages(response)
		if len(messages) == 0 {
			messages = []codexSearchMessage{{ID: "title", Text: session.Title}}
		}
		for _, message := range messages {
			for part, chunk := range searchindex.ChunkText(message.Text) {
				chunkID := fmt.Sprintf("%s-%d", message.ID, part)
				doc := apps.SearchDocument{
					ObjectID: session.ID, ChunkID: chunkID, Title: session.Title,
					Subtitle: message.Role, Keywords: session.Model + " " + message.Role, Body: chunk,
					URL: "/codex", Anchor: "session-" + session.ID, ModifiedAt: session.UpdatedAt,
				}
				if err := sink.AddSearchDocument(ctx, doc); err != nil {
					return err
				}
			}
		}
	}
	return nil
}

type codexSearchMessage struct {
	ID, Role, Text string
}

func collectCodexMessages(value any) []codexSearchMessage {
	data, _ := json.Marshal(value)
	var decoded any
	_ = json.Unmarshal(data, &decoded)
	var out []codexSearchMessage
	var visit func(any)
	visit = func(node any) {
		switch node := node.(type) {
		case []any:
			for _, item := range node {
				visit(item)
			}
		case map[string]any:
			kind := strings.ToLower(stringValue(node["type"]))
			text := firstCodexText(node)
			if text != "" && (strings.Contains(kind, "user") || strings.Contains(kind, "agent") || strings.Contains(kind, "assistant")) {
				role := "Assistant"
				if strings.Contains(kind, "user") {
					role = "You"
				}
				id := firstString(node, "id", "itemId", "item_id")
				if id == "" {
					id = fmt.Sprint(len(out))
				}
				out = append(out, codexSearchMessage{ID: id, Role: role, Text: text})
				return
			}
			for _, child := range node {
				visit(child)
			}
		}
	}
	visit(decoded)
	return out
}

func firstCodexText(node map[string]any) string {
	for _, key := range []string{"text", "message"} {
		if value := stringValue(node[key]); value != "" {
			return value
		}
	}
	switch content := node["content"].(type) {
	case string:
		return content
	case []any:
		var parts []string
		for _, item := range content {
			if nested, ok := item.(map[string]any); ok {
				if value := firstCodexText(nested); value != "" {
					parts = append(parts, value)
				}
			}
		}
		return strings.Join(parts, "\n")
	}
	return ""
}

func firstString(node map[string]any, keys ...string) string {
	for _, key := range keys {
		if value := stringValue(node[key]); value != "" {
			return value
		}
	}
	return ""
}

func stringValue(value any) string {
	valueString, _ := value.(string)
	return valueString
}
