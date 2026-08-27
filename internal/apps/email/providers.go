package email

import (
	"context"
	"fmt"
	"net/url"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/preferences"
)

type mailWidgetSettings struct {
	AccountID string `json:"account_id"`
}

func (a *App) DashboardWidgets() []apps.WidgetDefinition {
	return []apps.WidgetDefinition{{ID: "email.unread", Title: "Mail", Icon: "fa-solid fa-envelope", Kind: "list", DefaultSize: apps.WidgetStandard, SupportedSizes: apps.AllDashboardWidgetSizes(), EmptyState: "No mail in this inbox", Configurable: true, Actions: []apps.WidgetAction{{ID: "open", Label: "Open Email", URL: "/email", Icon: "fa-solid fa-arrow-up-right-from-square"}}}}
}

func (a *App) DashboardWidget(ctx context.Context, request apps.DashboardWidgetRequest, id string) (apps.WidgetData, error) {
	if id != "email.unread" {
		return apps.WidgetData{}, fmt.Errorf("unknown email widget")
	}
	settings, err := a.loadMailWidgetSettings(ctx, request.UserID)
	if err != nil {
		return apps.WidgetData{}, err
	}
	limit := map[apps.WidgetSize]int{
		apps.WidgetCompact: 1, apps.WidgetStandard: 3, apps.WidgetWide: 6, apps.WidgetLarge: 10,
	}[request.Size]
	threads, _, err := a.threads(ctx, request.UserID, threadQuery{AccountID: settings.AccountID, Role: "inbox", Limit: limit})
	if err != nil {
		return apps.WidgetData{}, err
	}
	items := make([]apps.WidgetItem, 0, len(threads))
	for _, t := range threads {
		values := url.Values{"thread": {t.ID}, "role": {"inbox"}}
		if settings.AccountID != "" {
			values.Set("account_id", settings.AccountID)
		}
		items = append(items, apps.WidgetItem{ID: t.ID, Title: t.Subject, Subtitle: t.Participants + " · " + relativeMailTime(t.LatestAt), URL: "/email?" + values.Encode(), Icon: "fa-regular fa-envelope", Favorite: t.Flagged})
	}
	def := a.DashboardWidgets()[0]
	actionURL := "/email?role=inbox"
	if settings.AccountID != "" {
		actionURL += "&account_id=" + url.QueryEscape(settings.AccountID)
	}
	actions := []apps.WidgetAction{{ID: "open", Label: "Open Email", URL: actionURL, Icon: "fa-solid fa-arrow-up-right-from-square"}}
	return apps.WidgetData{Kind: def.Kind, Items: items, Actions: actions, EmptyState: def.EmptyState}, nil
}

func (a *App) loadMailWidgetSettings(ctx context.Context, uid string) (mailWidgetSettings, error) {
	var settings mailWidgetSettings
	if a.db == nil {
		return settings, nil
	}
	conn, release, err := a.db.Conn(ctx)
	if err != nil {
		return settings, err
	}
	_, err = preferences.GetJSON(conn, uid, "email", "dashboard_widget", &settings)
	release()
	if err != nil || settings.AccountID == "" {
		return settings, err
	}
	if _, accountErr := a.account(ctx, uid, settings.AccountID); accountErr != nil {
		settings.AccountID = ""
	}
	return settings, nil
}

func (a *App) DashboardWidgetSettings(ctx context.Context, request apps.DashboardWidgetRequest, id string) (apps.WidgetSettings, error) {
	if id != "email.unread" {
		return apps.WidgetSettings{}, fmt.Errorf("widget settings not found")
	}
	settings, err := a.loadMailWidgetSettings(ctx, request.UserID)
	if err != nil {
		return apps.WidgetSettings{}, err
	}
	accounts, err := a.accounts(ctx, request.UserID)
	if err != nil {
		return apps.WidgetSettings{}, err
	}
	options := []apps.WidgetSettingOption{{Value: "", Label: "All inboxes"}}
	for _, account := range accounts {
		label := account.Email
		if account.Name != "" {
			label = account.Name + " — " + account.Email
		}
		options = append(options, apps.WidgetSettingOption{Value: account.ID, Label: label})
	}
	return apps.WidgetSettings{Fields: []apps.WidgetSettingField{{
		Name: "account_id", Label: "Inbox", Type: "select", Value: settings.AccountID, Options: options,
		Help: "Choose one account Inbox, or combine every Inbox.",
	}}}, nil
}

func (a *App) SaveDashboardWidgetSettings(ctx context.Context, request apps.DashboardWidgetRequest, id string, values map[string]string) (apps.WidgetSettings, error) {
	if id != "email.unread" {
		return apps.WidgetSettings{}, fmt.Errorf("widget settings not found")
	}
	accountID := values["account_id"]
	if accountID != "" {
		if _, err := a.account(ctx, request.UserID, accountID); err != nil {
			return apps.WidgetSettings{}, fmt.Errorf("invalid inbox")
		}
	}
	if a.db == nil {
		return apps.WidgetSettings{}, fmt.Errorf("mail storage unavailable")
	}
	conn, release, err := a.db.Conn(ctx)
	if err != nil {
		return apps.WidgetSettings{}, err
	}
	err = preferences.SetJSON(conn, request.UserID, "email", "dashboard_widget", mailWidgetSettings{AccountID: accountID})
	release()
	if err != nil {
		return apps.WidgetSettings{}, err
	}
	return a.DashboardWidgetSettings(ctx, request, id)
}

func relativeMailTime(ts int64) string {
	if ts == 0 {
		return ""
	}
	d := time.Since(time.Unix(ts, 0))
	switch {
	case d < time.Minute:
		return "now"
	case d < time.Hour:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	case d < 24*time.Hour:
		return fmt.Sprintf("%dh", int(d.Hours()))
	case d < 7*24*time.Hour:
		return fmt.Sprintf("%dd", int(d.Hours()/24))
	default:
		return time.Unix(ts, 0).Format("Jan 2")
	}
}
