// Package apps defines the App interface and registry for Dar apps.
package apps

import (
	"context"
	"encoding/json"
	"fmt"
	"html/template"
	"net/http"
	"sort"
	"strings"
)

// AuthMode controls the authentication behavior applied to an app route.
type AuthMode uint8

const (
	AuthPublic AuthMode = iota + 1
	AuthBrowser
	AuthAPI
	// AuthSessionAPI permits browser OIDC sessions only. Mutating requests are
	// protected by the server's same-origin and CSRF checks.
	AuthSessionAPI
)

// MethodAny registers a handler for every HTTP method.
const MethodAny = "*"

// Route is an HTTP route registered by an app on the main server.
type Route struct {
	Method  string
	Pattern string
	Auth    AuthMode
	Handler http.Handler
}

// App is the interface that every Dar app must implement.
type App interface {
	Name() string
	DisplayName() string
	Icon() string
	Description() string
	AppBarOrder() int
	Routes() []Route
}

// Lifecycle is implemented by apps that run background work.
type Lifecycle interface {
	Start(context.Context) error
	Shutdown(context.Context) error
}

// SearchDocument is one independently searchable chunk of an app object.
// ObjectID identifies the parent result while ChunkID and Anchor identify the
// exact matching region inside it.
type SearchDocument struct {
	ObjectID   string
	ChunkID    string
	Title      string
	Subtitle   string
	Keywords   string
	Body       string
	URL        string
	Anchor     string
	ModifiedAt int64
}

// SearchDocumentSink receives a complete provider snapshot during a
// reconciliation run.
type SearchDocumentSink interface {
	AddSearchDocument(context.Context, SearchDocument) error
}

// SearchIndexer is implemented by every app that participates in unified
// search.
type SearchIndexer interface {
	IndexSearch(context.Context, string, SearchDocumentSink) error
}

// SearchIndexWriter is scoped to one provider by the search service. Apps use
// it after authoritative writes so search freshness never depends on a full
// reconciliation.
type SearchIndexWriter interface {
	ReplaceSearchObject(context.Context, string, string, []SearchDocument) error
	DeleteSearchObject(context.Context, string, string) error
	MarkSearchDirty(string)
}

// AssistantEffect describes the side effect of an assistant tool. The Codex
// app uses it to enforce confirmation policy independently from model output.
type AssistantEffect string

const (
	AssistantRead        AssistantEffect = "read"
	AssistantSafeWrite   AssistantEffect = "safe_write"
	AssistantExternal    AssistantEffect = "external"
	AssistantDestructive AssistantEffect = "destructive"
)

// AssistantTool is a typed function exposed to the Codex app-server. Name is
// local to the provider namespace; InputSchema must be a JSON object schema.
type AssistantTool struct {
	Name        string          `json:"name"`
	Description string          `json:"description"`
	InputSchema json.RawMessage `json:"inputSchema"`
	Effect      AssistantEffect `json:"-"`
}

// AssistantContent is returned by a Dar assistant tool. ImageURL may be a data
// URL or another URL accepted by the Codex app-server protocol.
type AssistantContent struct {
	Type     string `json:"type"`
	Text     string `json:"text,omitempty"`
	ImageURL string `json:"image_url,omitempty"`
}

type AssistantResult struct {
	Content []AssistantContent `json:"content"`
}

type AssistantErrorCategory string

const (
	AssistantInvalid     AssistantErrorCategory = "invalid"
	AssistantNotFound    AssistantErrorCategory = "not_found"
	AssistantForbidden   AssistantErrorCategory = "forbidden"
	AssistantConflict    AssistantErrorCategory = "conflict"
	AssistantUnavailable AssistantErrorCategory = "unavailable"
	AssistantInternal    AssistantErrorCategory = "internal"
)

// AssistantError gives Codex a stable, non-secret failure category while
// preserving a concise user-facing explanation.
type AssistantError struct {
	Category AssistantErrorCategory
	Message  string
}

func (e *AssistantError) Error() string { return string(e.Category) + ": " + e.Message }

// AssistantProvider exposes user-scoped operations without coupling the Codex
// integration to an app's HTTP handlers or storage representation.
type AssistantProvider interface {
	AssistantTools() []AssistantTool
	CallAssistantTool(context.Context, string, string, json.RawMessage) (AssistantResult, error)
}

// RecipientSuggestion is a lightweight address suggestion shared between apps.
type RecipientSuggestion struct {
	Name  string `json:"name,omitempty"`
	Email string `json:"email"`
}

// RecipientProvider exposes user-scoped address suggestions without coupling
// consumers to an app's storage format.
type RecipientProvider interface {
	SuggestRecipients(context.Context, string, string, int) ([]RecipientSuggestion, error)
}

type WidgetSize string

const (
	WidgetCompact  WidgetSize = "compact"
	WidgetStandard WidgetSize = "standard"
	WidgetWide     WidgetSize = "wide"
	WidgetLarge    WidgetSize = "large"
)

type WidgetPreset struct {
	Size    WidgetSize `json:"size"`
	Label   string     `json:"label"`
	W       int        `json:"w"`
	H       int        `json:"h"`
	MobileH int        `json:"mobileH"`
}

var dashboardWidgetPresets = []WidgetPreset{
	{Size: WidgetCompact, Label: "Compact", W: 3, H: 2, MobileH: 2},
	{Size: WidgetStandard, Label: "Standard", W: 4, H: 3, MobileH: 4},
	{Size: WidgetWide, Label: "Wide", W: 6, H: 4, MobileH: 6},
	{Size: WidgetLarge, Label: "Large", W: 8, H: 6, MobileH: 8},
}

// DashboardWidgetPresets returns the global semantic dashboard sizes.
func DashboardWidgetPresets() []WidgetPreset {
	return append([]WidgetPreset(nil), dashboardWidgetPresets...)
}

// DashboardWidgetPreset returns the dimensions for a semantic size.
func DashboardWidgetPreset(size WidgetSize) (WidgetPreset, bool) {
	for _, preset := range dashboardWidgetPresets {
		if preset.Size == size {
			return preset, true
		}
	}
	return WidgetPreset{}, false
}

// AllDashboardWidgetSizes returns every currently supported global size.
func AllDashboardWidgetSizes() []WidgetSize {
	out := make([]WidgetSize, 0, len(dashboardWidgetPresets))
	for _, preset := range dashboardWidgetPresets {
		out = append(out, preset.Size)
	}
	return out
}

type WidgetAction struct {
	ID    string `json:"id"`
	Label string `json:"label"`
	URL   string `json:"url"`
	Icon  string `json:"icon,omitempty"`
	Kind  string `json:"kind,omitempty"`
}
type WidgetDefinition struct {
	ID             string         `json:"id"`
	Title          string         `json:"title"`
	Icon           string         `json:"icon"`
	Kind           string         `json:"kind"`
	DefaultSize    WidgetSize     `json:"defaultSize"`
	SupportedSizes []WidgetSize   `json:"supportedSizes"`
	EmptyState     string         `json:"emptyState"`
	Actions        []WidgetAction `json:"actions,omitempty"`
	Configurable   bool           `json:"configurable,omitempty"`
}
type WidgetItem struct {
	ID       string `json:"id"`
	Title    string `json:"title"`
	Subtitle string `json:"subtitle,omitempty"`
	URL      string `json:"url"`
	Icon     string `json:"icon,omitempty"`
	Favorite bool   `json:"favorite,omitempty"`
}
type WidgetSection struct {
	Title string       `json:"title"`
	Items []WidgetItem `json:"items"`
}
type WidgetWeatherCurrent struct {
	Temperature float64 `json:"temperature"`
	Apparent    float64 `json:"apparent"`
	Code        int     `json:"code"`
	Label       string  `json:"label"`
	Icon        string  `json:"icon"`
	IsDay       bool    `json:"isDay"`
}
type WidgetWeatherDay struct {
	Date  string  `json:"date"`
	Low   float64 `json:"low"`
	High  float64 `json:"high"`
	Code  int     `json:"code"`
	Label string  `json:"label"`
	Icon  string  `json:"icon"`
}
type WidgetWeather struct {
	Location  string               `json:"location"`
	Timezone  string               `json:"timezone"`
	Unit      string               `json:"unit"`
	Current   WidgetWeatherCurrent `json:"current"`
	Forecast  []WidgetWeatherDay   `json:"forecast"`
	UpdatedAt int64                `json:"updatedAt"`
	Stale     bool                 `json:"stale,omitempty"`
}
type WidgetData struct {
	Kind       string          `json:"kind"`
	Items      []WidgetItem    `json:"items"`
	Sections   []WidgetSection `json:"sections,omitempty"`
	Weather    *WidgetWeather  `json:"weather,omitempty"`
	Actions    []WidgetAction  `json:"actions,omitempty"`
	EmptyState string          `json:"emptyState"`
}
type DashboardWidgetRequest struct {
	UserID   string
	Timezone string
	Size     WidgetSize
}
type DashboardProvider interface {
	DashboardWidgets() []WidgetDefinition
	DashboardWidget(context.Context, DashboardWidgetRequest, string) (WidgetData, error)
}
type WidgetSettingOption struct {
	Value string `json:"value"`
	Label string `json:"label"`
}
type WidgetSettingField struct {
	Name        string                `json:"name"`
	Label       string                `json:"label"`
	Type        string                `json:"type"`
	Value       string                `json:"value"`
	Placeholder string                `json:"placeholder,omitempty"`
	Help        string                `json:"help,omitempty"`
	Required    bool                  `json:"required,omitempty"`
	Options     []WidgetSettingOption `json:"options,omitempty"`
}
type WidgetSettings struct {
	Fields  []WidgetSettingField `json:"fields"`
	Warning string               `json:"warning,omitempty"`
}
type DashboardSettingsProvider interface {
	DashboardWidgetSettings(context.Context, DashboardWidgetRequest, string) (WidgetSettings, error)
	SaveDashboardWidgetSettings(context.Context, DashboardWidgetRequest, string, map[string]string) (WidgetSettings, error)
}

// ActivityEvent is a plain-text, non-sensitive description of one completed
// action. RecipientIDs is a snapshot of every user allowed to see the event.
type ActivityEvent struct {
	ID           string            `json:"id,omitempty"`
	ActorID      string            `json:"actor_id,omitempty"`
	ActorName    string            `json:"actor_name"`
	ActorKind    string            `json:"actor_kind"`
	Source       string            `json:"source"`
	Action       string            `json:"action"`
	Summary      string            `json:"summary"`
	ObjectType   string            `json:"object_type,omitempty"`
	ObjectID     string            `json:"object_id,omitempty"`
	ObjectName   string            `json:"object_name,omitempty"`
	ObjectURL    string            `json:"object_url,omitempty"`
	Origin       string            `json:"origin"`
	HappenedAt   int64             `json:"happened_at,omitempty"`
	RecipientIDs []string          `json:"recipient_ids"`
	Metadata     map[string]string `json:"metadata,omitempty"`
}

// ActivityRecorder is implemented by the Activity app and discovered through
// Registry. Apps call Registry.EnqueueActivity rather than depending on the
// concrete Activity package.
type ActivityRecorder interface {
	EnqueueActivity(context.Context, ActivityEvent) error
}

// Notification is one user-scoped, actionable item shown in Dar's global
// notification center. DedupeKey is stable within Source and RecipientID so
// producers can retry delivery without creating duplicates.
type Notification struct {
	ID          string `json:"id,omitempty"`
	RecipientID string `json:"recipient_id,omitempty"`
	Source      string `json:"source"`
	Kind        string `json:"kind"`
	Subject     string `json:"subject"`
	Message     string `json:"message,omitempty"`
	Icon        string `json:"icon,omitempty"`
	ObjectURL   string `json:"object_url,omitempty"`
	DedupeKey   string `json:"dedupe_key,omitempty"`
	CreatedAt   int64  `json:"created_at,omitempty"`
}

// NotificationRecorder is implemented by Dar's headless Notifications
// service. Apps enqueue notifications through Registry so they do not depend
// on the concrete storage or live-delivery implementation.
type NotificationRecorder interface {
	EnqueueNotification(context.Context, Notification) error
}

// DashboardError lets a provider return a safe HTTP status without exposing
// implementation details through the dashboard API.
type DashboardError struct {
	Status int
	Err    error
}

func (e *DashboardError) Error() string {
	if e == nil || e.Err == nil {
		return "dashboard error"
	}
	return e.Err.Error()
}

func (e *DashboardError) Unwrap() error { return e.Err }

// Registry collects and orders apps for the app bar and home screen.
type Registry struct {
	apps                 []App
	started              []Lifecycle
	assistantProviders   map[string]AssistantProvider
	dashboardProviders   []registeredDashboardProvider
	activityRecorder     ActivityRecorder
	notificationRecorder NotificationRecorder
}

type registeredDashboardProvider struct {
	order    int
	provider DashboardProvider
}

// NewRegistry creates a new app registry.
func NewRegistry() *Registry {
	return &Registry{assistantProviders: make(map[string]AssistantProvider)}
}

// Register adds an app, inserting it in AppBarOrder-sorted position.
func (r *Registry) Register(app App) {
	if recorder, ok := app.(ActivityRecorder); ok {
		if r.activityRecorder != nil {
			panic("activity recorder is already registered")
		}
		r.activityRecorder = recorder
	}
	if provider, ok := app.(DashboardProvider); ok {
		r.ensureUniqueDashboardWidgets(provider)
	}
	idx := sort.Search(len(r.apps), func(i int) bool {
		return r.apps[i].AppBarOrder() >= app.AppBarOrder()
	})
	r.apps = append(r.apps, nil)
	copy(r.apps[idx+1:], r.apps[idx:])
	r.apps[idx] = app
	if provider, ok := app.(AssistantProvider); ok {
		r.assistantProviders[app.Name()] = provider
	}
}

// EnqueueActivity validates an event and delegates it to the registered
// Activity app. Callers should report errors internally without rolling back an
// already-completed primary operation.
func (r *Registry) EnqueueActivity(ctx context.Context, event ActivityEvent) error {
	if r == nil || r.activityRecorder == nil {
		return fmt.Errorf("activity recorder is not registered")
	}
	event.Source = strings.TrimSpace(event.Source)
	event.Action = strings.TrimSpace(event.Action)
	event.Summary = strings.TrimSpace(event.Summary)
	event.ActorName = strings.TrimSpace(event.ActorName)
	event.ActorKind = strings.TrimSpace(event.ActorKind)
	event.Origin = strings.TrimSpace(event.Origin)
	if event.Source == "" || event.Action == "" || event.Summary == "" {
		return fmt.Errorf("activity source, action, and summary are required")
	}
	if event.Source == "activity" {
		return fmt.Errorf("activity events must use an app or system source")
	}
	if event.Source != "system" {
		if app, ok := r.App(event.Source); !ok || app.Name() == "activity" {
			return fmt.Errorf("unknown activity source %q", event.Source)
		}
	}
	if event.ActorName == "" || event.ActorKind == "" || event.Origin == "" {
		return fmt.Errorf("activity actor name, actor kind, and origin are required")
	}
	if event.ObjectURL != "" && (!strings.HasPrefix(event.ObjectURL, "/") || strings.HasPrefix(event.ObjectURL, "//")) {
		return fmt.Errorf("activity object URL must be a same-origin path")
	}
	seen := make(map[string]bool, len(event.RecipientIDs))
	recipients := event.RecipientIDs[:0]
	for _, id := range event.RecipientIDs {
		id = strings.TrimSpace(id)
		if id != "" && !seen[id] {
			seen[id] = true
			recipients = append(recipients, id)
		}
	}
	if len(recipients) == 0 {
		return fmt.Errorf("activity requires at least one recipient")
	}
	event.RecipientIDs = recipients
	return r.activityRecorder.EnqueueActivity(ctx, event)
}

// RegisterNotificationRecorder installs the headless notification service.
// It is registered separately from apps because Notifications has global shell
// UI but no launcher entry of its own.
func (r *Registry) RegisterNotificationRecorder(recorder NotificationRecorder) {
	if recorder == nil {
		panic("notification recorder is required")
	}
	if r.notificationRecorder != nil {
		panic("notification recorder is already registered")
	}
	r.notificationRecorder = recorder
}

// EnqueueNotification validates and delegates a notification to the registered
// headless service. As with activity events, producers should log delivery
// errors without rolling back their already-completed primary operation.
func (r *Registry) EnqueueNotification(ctx context.Context, notification Notification) error {
	if r == nil || r.notificationRecorder == nil {
		return fmt.Errorf("notification recorder is not registered")
	}
	notification.RecipientID = strings.TrimSpace(notification.RecipientID)
	notification.Source = strings.TrimSpace(notification.Source)
	notification.Kind = strings.TrimSpace(notification.Kind)
	notification.Subject = strings.TrimSpace(notification.Subject)
	notification.Message = strings.TrimSpace(notification.Message)
	notification.Icon = strings.TrimSpace(notification.Icon)
	notification.ObjectURL = strings.TrimSpace(notification.ObjectURL)
	notification.DedupeKey = strings.TrimSpace(notification.DedupeKey)
	if notification.RecipientID == "" || notification.Source == "" || notification.Kind == "" || notification.Subject == "" {
		return fmt.Errorf("notification recipient, source, kind, and subject are required")
	}
	if app, ok := r.App(notification.Source); !ok || app.Name() == "activity" {
		return fmt.Errorf("unknown notification source %q", notification.Source)
	}
	if notification.ObjectURL != "" && (!strings.HasPrefix(notification.ObjectURL, "/") || strings.HasPrefix(notification.ObjectURL, "//")) {
		return fmt.Errorf("notification object URL must be a same-origin path")
	}
	return r.notificationRecorder.EnqueueNotification(ctx, notification)
}

// RegisterDashboardProvider adds a widget-only provider without adding an app
// menu entry. Providers are merged with app widgets by order.
func (r *Registry) RegisterDashboardProvider(order int, provider DashboardProvider) {
	if provider == nil {
		panic("dashboard provider is required")
	}
	r.ensureUniqueDashboardWidgets(provider)
	r.dashboardProviders = append(r.dashboardProviders, registeredDashboardProvider{order: order, provider: provider})
	sort.SliceStable(r.dashboardProviders, func(i, j int) bool {
		return r.dashboardProviders[i].order < r.dashboardProviders[j].order
	})
}

func (r *Registry) ensureUniqueDashboardWidgets(candidate DashboardProvider) {
	existing := map[string]bool{}
	for _, entry := range r.allDashboardProviders() {
		for _, definition := range entry.provider.DashboardWidgets() {
			existing[definition.ID] = true
		}
	}
	for _, definition := range candidate.DashboardWidgets() {
		if definition.ID == "" {
			panic("dashboard widget id is required")
		}
		if existing[definition.ID] {
			panic(fmt.Sprintf("dashboard widget %q is already registered", definition.ID))
		}
		if len(definition.SupportedSizes) == 0 {
			panic(fmt.Sprintf("dashboard widget %q must support at least one size", definition.ID))
		}
		sizes := make(map[WidgetSize]bool, len(definition.SupportedSizes))
		for _, size := range definition.SupportedSizes {
			if _, ok := DashboardWidgetPreset(size); !ok {
				panic(fmt.Sprintf("dashboard widget %q has unknown size %q", definition.ID, size))
			}
			if sizes[size] {
				panic(fmt.Sprintf("dashboard widget %q repeats size %q", definition.ID, size))
			}
			sizes[size] = true
		}
		if _, ok := DashboardWidgetPreset(definition.DefaultSize); !ok || !sizes[definition.DefaultSize] {
			panic(fmt.Sprintf("dashboard widget %q has unsupported default size %q", definition.ID, definition.DefaultSize))
		}
		existing[definition.ID] = true
	}
}

func (r *Registry) allDashboardProviders() []registeredDashboardProvider {
	out := append([]registeredDashboardProvider(nil), r.dashboardProviders...)
	for _, app := range r.apps {
		if provider, ok := app.(DashboardProvider); ok {
			out = append(out, registeredDashboardProvider{order: app.AppBarOrder(), provider: provider})
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].order < out[j].order })
	return out
}

// RegisterAssistantProvider adds a non-app provider such as Dar's core user
// settings. Provider namespaces share the same uniqueness rules as app names.
func (r *Registry) RegisterAssistantProvider(namespace string, provider AssistantProvider) {
	if namespace == "" || provider == nil {
		panic("assistant provider namespace and provider are required")
	}
	if _, exists := r.assistantProviders[namespace]; exists {
		panic(fmt.Sprintf("assistant provider %q is already registered", namespace))
	}
	r.assistantProviders[namespace] = provider
}

// AssistantProviders returns a defensive copy of all registered providers.
func (r *Registry) AssistantProviders() map[string]AssistantProvider {
	out := make(map[string]AssistantProvider, len(r.assistantProviders))
	for namespace, provider := range r.assistantProviders {
		out[namespace] = provider
	}
	return out
}

// Apps returns all registered apps in AppBarOrder.
func (r *Registry) Apps() []App {
	return append([]App(nil), r.apps...)
}

// App returns a registered app by name.
func (r *Registry) App(name string) (App, bool) {
	for _, app := range r.apps {
		if app.Name() == name {
			return app, true
		}
	}
	return nil, false
}

func (r *Registry) DashboardCatalog() []WidgetDefinition {
	var out []WidgetDefinition
	for _, entry := range r.allDashboardProviders() {
		out = append(out, entry.provider.DashboardWidgets()...)
	}
	return out
}
func (r *Registry) DashboardWidget(ctx context.Context, request DashboardWidgetRequest, id string) (WidgetData, error) {
	for _, entry := range r.allDashboardProviders() {
		for _, definition := range entry.provider.DashboardWidgets() {
			if definition.ID == id {
				if request.Size == "" {
					request.Size = definition.DefaultSize
				}
				if _, ok := DashboardWidgetPreset(request.Size); !ok || !widgetSupportsSize(definition, request.Size) {
					return WidgetData{}, &DashboardError{Status: http.StatusBadRequest, Err: fmt.Errorf("unsupported widget size")}
				}
				return entry.provider.DashboardWidget(ctx, request, id)
			}
		}
	}
	return WidgetData{}, fmt.Errorf("widget not found")
}

func widgetSupportsSize(definition WidgetDefinition, size WidgetSize) bool {
	for _, supported := range definition.SupportedSizes {
		if supported == size {
			return true
		}
	}
	return false
}

func (r *Registry) DashboardWidgetSettings(ctx context.Context, request DashboardWidgetRequest, id string) (WidgetSettings, error) {
	for _, entry := range r.allDashboardProviders() {
		for _, definition := range entry.provider.DashboardWidgets() {
			if definition.ID != id {
				continue
			}
			settings, ok := entry.provider.(DashboardSettingsProvider)
			if !ok || !definition.Configurable {
				return WidgetSettings{}, fmt.Errorf("widget settings not found")
			}
			return settings.DashboardWidgetSettings(ctx, request, id)
		}
	}
	return WidgetSettings{}, fmt.Errorf("widget not found")
}

func (r *Registry) SaveDashboardWidgetSettings(ctx context.Context, request DashboardWidgetRequest, id string, values map[string]string) (WidgetSettings, error) {
	for _, entry := range r.allDashboardProviders() {
		for _, definition := range entry.provider.DashboardWidgets() {
			if definition.ID != id {
				continue
			}
			settings, ok := entry.provider.(DashboardSettingsProvider)
			if !ok || !definition.Configurable {
				return WidgetSettings{}, fmt.Errorf("widget settings not found")
			}
			return settings.SaveDashboardWidgetSettings(ctx, request, id, values)
		}
	}
	return WidgetSettings{}, fmt.Errorf("widget not found")
}

// Start starts every app that implements Lifecycle. If startup fails, apps
// already started are shut down before the error is returned.
func (r *Registry) Start(ctx context.Context) error {
	for _, app := range r.apps {
		lifecycle, ok := app.(Lifecycle)
		if !ok {
			continue
		}
		if err := lifecycle.Start(ctx); err != nil {
			_ = r.Shutdown(context.Background())
			return fmt.Errorf("start app %q: %w", app.Name(), err)
		}
		r.started = append(r.started, lifecycle)
	}
	return nil
}

// Shutdown stops lifecycle apps in reverse startup order.
func (r *Registry) Shutdown(ctx context.Context) error {
	var firstErr error
	for i := len(r.started) - 1; i >= 0; i-- {
		if err := r.started[i].Shutdown(ctx); err != nil && firstErr == nil {
			firstErr = err
		}
	}
	r.started = nil
	return firstErr
}

// AppBarHTML returns the data-backed Focus application switcher.
func (r *Registry) AppBarHTML() template.HTML {
	html := `<div id="dar-app-switcher" class="relative">
		<button id="dar-app-menu-button" data-wallpaper-contrast type="button" aria-haspopup="menu" aria-expanded="false"
			class="focus-switcher hidden md:inline-flex h-10 max-w-52 px-3 rounded-xl items-center gap-2 text-sm border border-outline-variant bg-surface-container-low hover:bg-surface-container">
			<i class="fa-solid fa-table-cells-large text-primary" aria-hidden="true"></i>
			<span id="dar-current-app" class="hidden sm:block truncate">Home</span>
			<i class="fa-solid fa-chevron-down text-[10px] text-on-surface-variant" aria-hidden="true"></i>
			<span class="sr-only">Switch app</span>
		</button>
		<nav id="dar-app-menu" role="menu" aria-label="Applications"
			class="focus-menu hidden absolute left-0 top-12 w-72 max-h-[min(38rem,calc(100dvh-6rem))] overflow-y-auto rounded-2xl border border-outline-variant bg-surface-container-low p-2 shadow-2xl">
			<div class="px-3 pb-2 pt-1 text-[11px] font-semibold uppercase tracking-widest text-on-surface-variant">Switch app</div>
			<a role="menuitem" data-app-path="/home" data-app-label="Home" href="/home"
				class="flex h-11 items-center gap-3 rounded-xl px-3 text-sm text-on-surface no-underline hover:bg-surface-container">
				<i class="fa-solid fa-house w-6 text-center text-primary" aria-hidden="true"></i><span>Home</span>
			</a>`
	for _, app := range r.apps {
		name := template.HTMLEscapeString(app.Name())
		label := template.HTMLEscapeString(app.DisplayName())
		html += `<a role="menuitem" data-app-path="/` + name + `" data-app-label="` + label + `" href="/` + name + `"
			class="flex h-11 items-center gap-3 rounded-xl px-3 text-sm text-on-surface no-underline hover:bg-surface-container">
			<span class="w-6 text-center text-primary" aria-hidden="true">` + app.Icon() + `</span><span>` + label + `</span></a>`
	}
	html += `</nav></div>`
	return template.HTML(html)
}

// HomeCardsHTML returns the HTML for the home screen app launcher grid.
func (r *Registry) HomeCardsHTML() template.HTML {
	if len(r.apps) == 0 {
		return template.HTML(`<p class="text-on-surface-variant">No apps installed yet.</p>`)
	}
	html := `<div class="grid grid-cols-2 md:grid-cols-4 gap-4 max-w-2xl mx-auto">`
	for _, app := range r.apps {
		html += `<a href="/` + app.Name() + `"
			class="bg-surface-container rounded-md shadow-1 hover:shadow-2 transition-shadow p-6 text-center no-underline text-on-surface">
			<div class="text-[40px] mb-3">` + app.Icon() + `</div>
			<div class="font-medium text-base">` + app.DisplayName() + `</div>
			<div class="text-on-surface-variant text-sm mt-1">` + app.Description() + `</div>
		</a>`
	}
	html += `</div>`
	return template.HTML(html)
}
