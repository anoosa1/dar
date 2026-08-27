package apps

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"strings"
	"testing"
)

type testApp struct {
	name  string
	order int
}

func TestAppBarHTMLUsesAccessibleMenuInRegistryOrder(t *testing.T) {
	r := NewRegistry()
	r.Register(&testApp{name: "notes", order: 2})
	r.Register(&testApp{name: "files", order: 1})
	html := string(r.AppBarHTML())
	for _, want := range []string{
		`id="dar-app-menu-button"`,
		`data-wallpaper-contrast`,
		`aria-haspopup="menu"`,
		`role="menu"`,
		`data-app-path="/home"`,
		`data-app-path="/files"`,
		`data-app-path="/notes"`,
	} {
		if !strings.Contains(html, want) {
			t.Fatalf("app menu missing %q", want)
		}
	}
	if strings.Index(html, `data-app-path="/home"`) > strings.Index(html, `data-app-path="/files"`) ||
		strings.Index(html, `data-app-path="/files"`) > strings.Index(html, `data-app-path="/notes"`) {
		t.Fatal("app menu order is not Dashboard, Files, Notes")
	}
}

func (a *testApp) Name() string        { return a.name }
func (a *testApp) DisplayName() string { return a.name }
func (a *testApp) Icon() string        { return "" }
func (a *testApp) Description() string { return "" }
func (a *testApp) AppBarOrder() int    { return a.order }
func (a *testApp) Routes() []Route     { return nil }

type lifecycleApp struct {
	testApp
	events *[]string
	fail   bool
}

type testDashboardProvider struct {
	id string
}

type testActivityApp struct {
	testApp
	events []ActivityEvent
	err    error
}

type testNotificationRecorder struct {
	notifications []Notification
	err           error
}

func (r *testNotificationRecorder) EnqueueNotification(_ context.Context, notification Notification) error {
	r.notifications = append(r.notifications, notification)
	return r.err
}

func (a *testActivityApp) EnqueueActivity(_ context.Context, event ActivityEvent) error {
	a.events = append(a.events, event)
	return a.err
}

type staticDashboardProvider struct {
	definition WidgetDefinition
}

type configurableDashboardProvider struct{}

func (configurableDashboardProvider) DashboardWidgets() []WidgetDefinition {
	return []WidgetDefinition{{ID: "configurable", Title: "Configurable", DefaultSize: WidgetStandard, SupportedSizes: []WidgetSize{WidgetStandard}, Configurable: true}}
}
func (configurableDashboardProvider) DashboardWidget(context.Context, DashboardWidgetRequest, string) (WidgetData, error) {
	return WidgetData{Kind: "list"}, nil
}
func (configurableDashboardProvider) DashboardWidgetSettings(context.Context, DashboardWidgetRequest, string) (WidgetSettings, error) {
	return WidgetSettings{Fields: []WidgetSettingField{{Name: "value", Value: "old"}}}, nil
}
func (configurableDashboardProvider) SaveDashboardWidgetSettings(_ context.Context, _ DashboardWidgetRequest, _ string, values map[string]string) (WidgetSettings, error) {
	return WidgetSettings{Fields: []WidgetSettingField{{Name: "value", Value: values["value"]}}}, nil
}

type testAssistantProvider struct{}

func (testAssistantProvider) AssistantTools() []AssistantTool {
	return []AssistantTool{{Name: "read", InputSchema: json.RawMessage(`{"type":"object"}`), Effect: AssistantRead}}
}
func (testAssistantProvider) CallAssistantTool(context.Context, string, string, json.RawMessage) (AssistantResult, error) {
	return AssistantResult{}, nil
}

func (p *testDashboardProvider) DashboardWidgets() []WidgetDefinition {
	return []WidgetDefinition{{ID: p.id, Title: p.id, Kind: "list", DefaultSize: WidgetStandard, SupportedSizes: AllDashboardWidgetSizes()}}
}

func (p *testDashboardProvider) DashboardWidget(context.Context, DashboardWidgetRequest, string) (WidgetData, error) {
	return WidgetData{Kind: "list"}, nil
}

func (p *staticDashboardProvider) DashboardWidgets() []WidgetDefinition {
	return []WidgetDefinition{p.definition}
}

func (*staticDashboardProvider) DashboardWidget(context.Context, DashboardWidgetRequest, string) (WidgetData, error) {
	return WidgetData{}, nil
}

func (a *lifecycleApp) Start(context.Context) error {
	*a.events = append(*a.events, "start:"+a.name)
	if a.fail {
		return errors.New("start failed")
	}
	return nil
}

func (a *lifecycleApp) Shutdown(context.Context) error {
	*a.events = append(*a.events, "stop:"+a.name)
	return nil
}

func TestRegistrySortAndLookup(t *testing.T) {
	r := NewRegistry()
	r.Register(&testApp{name: "later", order: 2})
	r.Register(&testApp{name: "first", order: 1})
	if got := r.Apps(); len(got) != 2 || got[0].Name() != "first" {
		t.Fatalf("Apps = %#v", got)
	}
	if app, ok := r.App("later"); !ok || app.Name() != "later" {
		t.Fatalf("App lookup failed")
	}
	if _, ok := r.App("missing"); ok {
		t.Fatal("missing app found")
	}
}

func TestRegistryActivityDelegationAndValidation(t *testing.T) {
	registry := NewRegistry()
	recorder := &testActivityApp{testApp: testApp{name: "activity", order: 2}}
	registry.Register(recorder)
	registry.Register(&testApp{name: "files", order: 3})

	event := ActivityEvent{
		ActorID: "alice", ActorName: " Alice ", ActorKind: "user",
		Source: " files ", Action: " uploaded ", Summary: " uploaded a file ",
		ObjectURL: "/files?path=%2Freport.pdf", Origin: "browser",
		RecipientIDs: []string{"alice", "alice", "  bob  ", ""},
	}
	if err := registry.EnqueueActivity(context.Background(), event); err != nil {
		t.Fatal(err)
	}
	if len(recorder.events) != 1 {
		t.Fatalf("recorded %d events", len(recorder.events))
	}
	got := recorder.events[0]
	if got.Source != "files" || got.Action != "uploaded" || got.ActorName != "Alice" {
		t.Fatalf("event was not normalized: %#v", got)
	}
	if len(got.RecipientIDs) != 2 || got.RecipientIDs[0] != "alice" || got.RecipientIDs[1] != "bob" {
		t.Fatalf("recipients = %#v", got.RecipientIDs)
	}

	bad := []ActivityEvent{
		{ActorName: "Alice", ActorKind: "user", Source: "missing", Action: "x", Summary: "x", Origin: "api", RecipientIDs: []string{"alice"}},
		{ActorName: "Alice", ActorKind: "user", Source: "activity", Action: "x", Summary: "x", Origin: "api", RecipientIDs: []string{"alice"}},
		{ActorName: "Alice", ActorKind: "user", Source: "files", Action: "x", Summary: "x", ObjectURL: "https://example.com", Origin: "api", RecipientIDs: []string{"alice"}},
		{ActorName: "Alice", ActorKind: "user", Source: "files", Action: "x", Summary: "x", Origin: "api"},
	}
	for i, candidate := range bad {
		if err := registry.EnqueueActivity(context.Background(), candidate); err == nil {
			t.Fatalf("bad event %d was accepted", i)
		}
	}
}

func TestRegistryRejectsMissingAndDuplicateActivityProvider(t *testing.T) {
	if err := NewRegistry().EnqueueActivity(context.Background(), ActivityEvent{}); err == nil {
		t.Fatal("missing provider was accepted")
	}
	registry := NewRegistry()
	registry.Register(&testActivityApp{testApp: testApp{name: "activity", order: 2}})
	defer func() {
		if recover() == nil {
			t.Fatal("duplicate provider registration did not panic")
		}
	}()
	registry.Register(&testActivityApp{testApp: testApp{name: "other", order: 3}})
}

func TestRegistryNotificationDelegationAndValidation(t *testing.T) {
	registry := NewRegistry()
	recorder := &testNotificationRecorder{}
	registry.RegisterNotificationRecorder(recorder)
	registry.Register(&testApp{name: "email", order: 9})

	if err := registry.EnqueueNotification(context.Background(), Notification{
		RecipientID: " alice ", Source: " email ", Kind: " new_mail ",
		Subject: " New email ", ObjectURL: "/email?thread=one", DedupeKey: " one ",
	}); err != nil {
		t.Fatal(err)
	}
	if len(recorder.notifications) != 1 {
		t.Fatalf("recorded %d notifications", len(recorder.notifications))
	}
	got := recorder.notifications[0]
	if got.RecipientID != "alice" || got.Source != "email" || got.Kind != "new_mail" || got.Subject != "New email" || got.DedupeKey != "one" {
		t.Fatalf("notification was not normalized: %#v", got)
	}

	for i, candidate := range []Notification{
		{RecipientID: "alice", Source: "missing", Kind: "new_mail", Subject: "New"},
		{RecipientID: "alice", Source: "email", Kind: "", Subject: "New"},
		{RecipientID: "alice", Source: "email", Kind: "new_mail", Subject: ""},
		{RecipientID: "alice", Source: "email", Kind: "new_mail", Subject: "New", ObjectURL: "https://example.test"},
	} {
		if err := registry.EnqueueNotification(context.Background(), candidate); err == nil {
			t.Fatalf("bad notification %d was accepted", i)
		}
	}
}

func TestRegistryRejectsMissingAndDuplicateNotificationRecorder(t *testing.T) {
	if err := NewRegistry().EnqueueNotification(context.Background(), Notification{}); err == nil {
		t.Fatal("missing notification recorder was accepted")
	}
	registry := NewRegistry()
	registry.RegisterNotificationRecorder(&testNotificationRecorder{})
	defer func() {
		if recover() == nil {
			t.Fatal("duplicate notification recorder registration did not panic")
		}
	}()
	registry.RegisterNotificationRecorder(&testNotificationRecorder{})
}

func TestRegistryLifecycleOrder(t *testing.T) {
	var events []string
	r := NewRegistry()
	r.Register(&lifecycleApp{testApp: testApp{name: "one", order: 1}, events: &events})
	r.Register(&lifecycleApp{testApp: testApp{name: "two", order: 2}, events: &events})
	if err := r.Start(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := r.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	want := []string{"start:one", "start:two", "stop:two", "stop:one"}
	if len(events) != len(want) {
		t.Fatalf("events = %v", events)
	}
	for i := range want {
		if events[i] != want[i] {
			t.Fatalf("events = %v", events)
		}
	}
}

func TestRegistryRollsBackOnStartFailure(t *testing.T) {
	var events []string
	r := NewRegistry()
	r.Register(&lifecycleApp{testApp: testApp{name: "one", order: 1}, events: &events})
	r.Register(&lifecycleApp{testApp: testApp{name: "two", order: 2}, events: &events, fail: true})
	if err := r.Start(context.Background()); err == nil {
		t.Fatal("Start succeeded")
	}
	want := []string{"start:one", "start:two", "stop:one"}
	if len(events) != len(want) {
		t.Fatalf("events = %v", events)
	}
	for i := range want {
		if events[i] != want[i] {
			t.Fatalf("events = %v", events)
		}
	}
}

func TestRegistryStandaloneDashboardProviderDoesNotBecomeApp(t *testing.T) {
	r := NewRegistry()
	r.RegisterDashboardProvider(9, &testDashboardProvider{id: "weather.current"})
	if len(r.Apps()) != 0 {
		t.Fatalf("widget-only provider appeared in Apps: %#v", r.Apps())
	}
	catalog := r.DashboardCatalog()
	if len(catalog) != 1 || catalog[0].ID != "weather.current" {
		t.Fatalf("DashboardCatalog = %#v", catalog)
	}
	if _, err := r.DashboardWidget(context.Background(), DashboardWidgetRequest{UserID: "alice", Timezone: "UTC"}, "weather.current"); err != nil {
		t.Fatal(err)
	}
}

func TestRegistryRejectsDuplicateDashboardWidgetID(t *testing.T) {
	r := NewRegistry()
	r.RegisterDashboardProvider(1, &testDashboardProvider{id: "duplicate"})
	defer func() {
		if recover() == nil {
			t.Fatal("duplicate widget registration did not panic")
		}
	}()
	r.RegisterDashboardProvider(2, &testDashboardProvider{id: "duplicate"})
}

func TestRegistryValidatesDashboardWidgetSizes(t *testing.T) {
	tests := []WidgetDefinition{
		{ID: "missing-sizes", DefaultSize: WidgetStandard},
		{ID: "unknown-size", DefaultSize: WidgetStandard, SupportedSizes: []WidgetSize{WidgetStandard, "giant"}},
		{ID: "duplicate-size", DefaultSize: WidgetStandard, SupportedSizes: []WidgetSize{WidgetStandard, WidgetStandard}},
		{ID: "unsupported-default", DefaultSize: WidgetWide, SupportedSizes: []WidgetSize{WidgetStandard}},
	}
	for _, definition := range tests {
		t.Run(definition.ID, func(t *testing.T) {
			defer func() {
				if recover() == nil {
					t.Fatal("invalid widget definition did not panic")
				}
			}()
			NewRegistry().RegisterDashboardProvider(1, &staticDashboardProvider{definition: definition})
		})
	}
}

func TestRegistryCapabilitiesAndHelperBoundaries(t *testing.T) {
	presets := DashboardWidgetPresets()
	if len(presets) != 4 {
		t.Fatalf("presets = %+v", presets)
	}
	presets[0].Label = "mutated"
	if DashboardWidgetPresets()[0].Label == "mutated" {
		t.Fatal("dashboard presets were not defensively copied")
	}
	if _, ok := DashboardWidgetPreset("missing"); ok {
		t.Fatal("unknown dashboard preset found")
	}
	dashboardError := &DashboardError{Status: http.StatusBadRequest, Err: errors.New("bad setting")}
	if dashboardError.Error() != "bad setting" || !errors.Is(dashboardError, dashboardError.Err) {
		t.Fatalf("dashboard error = %v", dashboardError)
	}
	if (&DashboardError{}).Error() != "dashboard error" || (*DashboardError)(nil).Error() != "dashboard error" {
		t.Fatal("nil dashboard error is not safe")
	}
	assistantError := &AssistantError{Category: AssistantInvalid, Message: "bad input"}
	if assistantError.Error() != "invalid: bad input" {
		t.Fatalf("assistant error = %q", assistantError.Error())
	}

	registry := NewRegistry()
	registry.RegisterDashboardProvider(1, configurableDashboardProvider{})
	request := DashboardWidgetRequest{UserID: "alice"}
	settings, err := registry.DashboardWidgetSettings(context.Background(), request, "configurable")
	if err != nil || len(settings.Fields) != 1 {
		t.Fatalf("settings=%+v err=%v", settings, err)
	}
	saved, err := registry.SaveDashboardWidgetSettings(context.Background(), request, "configurable", map[string]string{"value": "new"})
	if err != nil || saved.Fields[0].Value != "new" {
		t.Fatalf("saved=%+v err=%v", saved, err)
	}
	if _, err := registry.DashboardWidget(context.Background(), DashboardWidgetRequest{Size: WidgetWide}, "configurable"); err == nil {
		t.Fatal("unsupported widget size was accepted")
	}
	if _, err := registry.DashboardWidget(context.Background(), request, "missing"); err == nil {
		t.Fatal("missing widget was found")
	}
	if _, err := registry.DashboardWidgetSettings(context.Background(), request, "missing"); err == nil {
		t.Fatal("missing widget settings were found")
	}
	if _, err := registry.SaveDashboardWidgetSettings(context.Background(), request, "missing", nil); err == nil {
		t.Fatal("missing widget settings were saved")
	}

	registry.RegisterAssistantProvider("core", testAssistantProvider{})
	providers := registry.AssistantProviders()
	if len(providers) != 1 || providers["core"] == nil {
		t.Fatalf("assistant providers = %+v", providers)
	}
	delete(providers, "core")
	if len(registry.AssistantProviders()) != 1 {
		t.Fatal("assistant provider map was not defensively copied")
	}
	for name, action := range map[string]func(){
		"nil dashboard":   func() { NewRegistry().RegisterDashboardProvider(1, nil) },
		"empty assistant": func() { NewRegistry().RegisterAssistantProvider("", testAssistantProvider{}) },
		"nil assistant":   func() { NewRegistry().RegisterAssistantProvider("core", nil) },
		"duplicate assistant": func() {
			value := NewRegistry()
			value.RegisterAssistantProvider("core", testAssistantProvider{})
			value.RegisterAssistantProvider("core", testAssistantProvider{})
		},
	} {
		t.Run(name, func(t *testing.T) {
			defer func() {
				if recover() == nil {
					t.Fatal("expected panic")
				}
			}()
			action()
		})
	}
	if !strings.Contains(string(NewRegistry().HomeCardsHTML()), "No apps") {
		t.Fatal("empty home card state missing")
	}
	registry.Register(&testApp{name: "files", order: 1})
	if !strings.Contains(string(registry.HomeCardsHTML()), `href="/files"`) {
		t.Fatal("registered app card missing")
	}
}
