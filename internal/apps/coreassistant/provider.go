package coreassistant

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
)

type Provider struct{ db *db.DB }

func New(database *db.DB) *Provider { return &Provider{db: database} }
func (p *Provider) Name() string    { return "core" }

func (p *Provider) AssistantTools() []apps.AssistantTool {
	return []apps.AssistantTool{
		{Name: "get_personal_profile", Description: "Read the user's non-secret personal profile and locale", InputSchema: coreSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "update_personal_profile", Description: "Update personal profile, language, or locale fields", InputSchema: coreSchema(`{"display_name":{"type":"string"},"phone":{"type":"string"},"address":{"type":"string"},"website":{"type":"string"},"twitter":{"type":"string"},"fediverse":{"type":"string"},"organisation":{"type":"string"},"role":{"type":"string"},"headline":{"type":"string"},"biography":{"type":"string"},"location":{"type":"string"},"language":{"type":"string"},"locale":{"type":"string"},"birthday":{"type":"string"}}`), Effect: apps.AssistantSafeWrite},
		{Name: "get_time_format", Description: "Read the user's clock-format preference", InputSchema: coreSchema(`{}`), Effect: apps.AssistantRead},
		{Name: "set_time_format", Description: "Set the user's 12-hour or 24-hour clock preference", InputSchema: coreSchema(`{"time_format_24h":{"type":"boolean"}}`, "time_format_24h"), Effect: apps.AssistantSafeWrite},
	}
}

func (p *Provider) CallAssistantTool(ctx context.Context, userID, name string, arguments json.RawMessage) (apps.AssistantResult, error) {
	if p.db == nil {
		return apps.AssistantResult{}, fmt.Errorf("database unavailable")
	}
	conn, release, err := p.db.Conn(ctx)
	if err != nil {
		return apps.AssistantResult{}, err
	}
	defer release()
	switch name {
	case "get_personal_profile":
		user, err := auth.GetUserByID(conn, userID)
		if err != nil || user == nil {
			return apps.AssistantResult{}, fmt.Errorf("user not found")
		}
		return coreResult(map[string]any{"display_name": user.DisplayName, "email": user.Email, "profile": user.Profile}), nil
	case "update_personal_profile":
		user, err := auth.GetUserByID(conn, userID)
		if err != nil || user == nil {
			return apps.AssistantResult{}, fmt.Errorf("user not found")
		}
		var values map[string]string
		if err := json.Unmarshal(arguments, &values); err != nil {
			return apps.AssistantResult{}, fmt.Errorf("invalid arguments: %w", err)
		}
		displayName := user.DisplayName
		if value, ok := values["display_name"]; ok {
			displayName = strings.TrimSpace(value)
		}
		profile := user.Profile
		setProfileFields(&profile, values)
		if err := auth.UpdateUserProfile(conn, userID, displayName, profile); err != nil {
			return apps.AssistantResult{}, err
		}
		return coreResult(map[string]any{"display_name": displayName, "profile": profile}), nil
	case "get_time_format":
		value := struct {
			TimeFormat24h bool `json:"time_format_24h"`
		}{}
		if _, err := preferences.GetJSON(conn, userID, "global", "time_format", &value); err != nil {
			return apps.AssistantResult{}, err
		}
		return coreResult(value), nil
	case "set_time_format":
		var value struct {
			TimeFormat24h bool `json:"time_format_24h"`
		}
		if err := json.Unmarshal(arguments, &value); err != nil {
			return apps.AssistantResult{}, fmt.Errorf("invalid arguments: %w", err)
		}
		if err := preferences.SetJSON(conn, userID, "global", "time_format", value); err != nil {
			return apps.AssistantResult{}, err
		}
		return coreResult(value), nil
	default:
		return apps.AssistantResult{}, fmt.Errorf("unknown core settings tool %q", name)
	}
}

func setProfileFields(profile *auth.UserProfile, values map[string]string) {
	set := func(key string, target *string) {
		if value, ok := values[key]; ok {
			*target = strings.TrimSpace(value)
		}
	}
	set("phone", &profile.Phone)
	set("address", &profile.Address)
	set("website", &profile.Website)
	set("twitter", &profile.Twitter)
	set("fediverse", &profile.Fediverse)
	set("organisation", &profile.Organisation)
	set("role", &profile.Role)
	set("headline", &profile.Headline)
	set("biography", &profile.Biography)
	set("location", &profile.Location)
	set("language", &profile.Language)
	set("locale", &profile.Locale)
	set("birthday", &profile.Birthday)
}

func coreResult(value any) apps.AssistantResult {
	data, _ := json.Marshal(value)
	return apps.AssistantResult{Content: []apps.AssistantContent{{Type: "text", Text: string(data)}}}
}
func coreSchema(properties string, required ...string) json.RawMessage {
	value := map[string]any{"type": "object", "properties": json.RawMessage(properties), "additionalProperties": false}
	if len(required) > 0 {
		value["required"] = required
	}
	data, _ := json.Marshal(value)
	return data
}
