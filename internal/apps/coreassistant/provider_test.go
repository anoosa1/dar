package coreassistant

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/testutil"
)

func resultObject(t *testing.T, result apps.AssistantResult) map[string]any {
	t.Helper()
	if len(result.Content) != 1 || result.Content[0].Type != "text" {
		t.Fatalf("unexpected result: %#v", result)
	}
	var value map[string]any
	if err := json.Unmarshal([]byte(result.Content[0].Text), &value); err != nil {
		t.Fatalf("decode result %q: %v", result.Content[0].Text, err)
	}
	return value
}

func TestProviderMetadataAndSchemas(t *testing.T) {
	provider := New(nil)
	if provider.Name() != "core" {
		t.Fatalf("name = %q", provider.Name())
	}
	tools := provider.AssistantTools()
	if len(tools) != 4 {
		t.Fatalf("tools = %d", len(tools))
	}
	wantEffects := map[string]apps.AssistantEffect{
		"get_personal_profile":    apps.AssistantRead,
		"update_personal_profile": apps.AssistantSafeWrite,
		"get_time_format":         apps.AssistantRead,
		"set_time_format":         apps.AssistantSafeWrite,
	}
	for _, tool := range tools {
		if wantEffects[tool.Name] != tool.Effect {
			t.Errorf("%s effect = %q", tool.Name, tool.Effect)
		}
		var schema map[string]any
		if err := json.Unmarshal(tool.InputSchema, &schema); err != nil {
			t.Errorf("%s invalid schema: %v", tool.Name, err)
			continue
		}
		if schema["type"] != "object" || schema["additionalProperties"] != false {
			t.Errorf("%s schema = %#v", tool.Name, schema)
		}
	}
}

func TestProviderProfileAndTimeFormatRoundTrip(t *testing.T) {
	migrations := append(auth.Migrations(), preferences.Migrations()...)
	database := testutil.OpenDB(t, migrations...)
	user := testutil.User(t, database, "core")
	provider := New(database)
	ctx := context.Background()

	initial, err := provider.CallAssistantTool(ctx, user.ID, "get_personal_profile", json.RawMessage(`{}`))
	if err != nil {
		t.Fatal(err)
	}
	if resultObject(t, initial)["email"] != user.Email {
		t.Fatalf("initial profile = %#v", resultObject(t, initial))
	}

	update, err := provider.CallAssistantTool(ctx, user.ID, "update_personal_profile", json.RawMessage(`{
		"display_name":"  Ada Lovelace  ",
		"phone":"  +1 555 0100  ",
		"address":" London ",
		"website":" https://example.test ",
		"twitter":" @ada ",
		"fediverse":" @ada@example.social ",
		"organisation":" Analytical Engines ",
		"role":" Programmer ",
		"headline":" First programmer ",
		"biography":" Notes ",
		"location":" Toronto ",
		"language":" en ",
		"locale":" en_CA ",
		"birthday":" 1815-12-10 "
	}`))
	if err != nil {
		t.Fatal(err)
	}
	updated := resultObject(t, update)
	if updated["display_name"] != "Ada Lovelace" {
		t.Fatalf("display name = %#v", updated["display_name"])
	}
	profile, ok := updated["profile"].(map[string]any)
	if !ok || profile["phone"] != "+1 555 0100" || profile["birthday"] != "1815-12-10" {
		t.Fatalf("profile = %#v", updated["profile"])
	}

	readBack, err := provider.CallAssistantTool(ctx, user.ID, "get_personal_profile", nil)
	if err != nil {
		t.Fatal(err)
	}
	if resultObject(t, readBack)["display_name"] != "Ada Lovelace" {
		t.Fatalf("read back = %#v", resultObject(t, readBack))
	}

	defaultFormat, err := provider.CallAssistantTool(ctx, user.ID, "get_time_format", nil)
	if err != nil {
		t.Fatal(err)
	}
	if value := resultObject(t, defaultFormat)["time_format_24h"]; value != false {
		t.Fatalf("default time format = %#v", value)
	}
	if _, err := provider.CallAssistantTool(ctx, user.ID, "set_time_format", json.RawMessage(`{"time_format_24h":true}`)); err != nil {
		t.Fatal(err)
	}
	storedFormat, err := provider.CallAssistantTool(ctx, user.ID, "get_time_format", nil)
	if err != nil {
		t.Fatal(err)
	}
	if value := resultObject(t, storedFormat)["time_format_24h"]; value != true {
		t.Fatalf("stored time format = %#v", value)
	}
}

func TestProviderRejectsInvalidCalls(t *testing.T) {
	if _, err := New(nil).CallAssistantTool(context.Background(), "missing", "get_personal_profile", nil); err == nil || !strings.Contains(err.Error(), "database unavailable") {
		t.Fatalf("nil database error = %v", err)
	}

	migrations := append(auth.Migrations(), preferences.Migrations()...)
	database := testutil.OpenDB(t, migrations...)
	provider := New(database)
	for _, tc := range []struct {
		name string
		tool string
		args string
		want string
	}{
		{"missing profile", "get_personal_profile", `{}`, "user not found"},
		{"missing update", "update_personal_profile", `{}`, "user not found"},
		{"unknown", "does_not_exist", `{}`, "unknown core settings tool"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			_, err := provider.CallAssistantTool(context.Background(), "missing", tc.tool, json.RawMessage(tc.args))
			if err == nil || !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("error = %v, want %q", err, tc.want)
			}
		})
	}

	user := testutil.User(t, database, "invalid-core")
	for _, tc := range []struct {
		tool string
		args string
	}{
		{"update_personal_profile", `[`},
		{"set_time_format", `[`},
	} {
		if _, err := provider.CallAssistantTool(context.Background(), user.ID, tc.tool, json.RawMessage(tc.args)); err == nil || !strings.Contains(err.Error(), "invalid arguments") {
			t.Errorf("%s malformed arguments error = %v", tc.tool, err)
		}
	}
}
