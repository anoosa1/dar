package files

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
)

func TestFavoriteFilesWidgetSkipsMissingEntriesAndUsesFileDeepLink(t *testing.T) {
	app := sqliteTestApp(t)
	if err := os.MkdirAll(app.userHome("alice"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(app.userHome("alice"), "favorite.txt"), []byte("hello"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := app.SetFavorite(context.Background(), "alice", "/favorite.txt", true); err != nil {
		t.Fatal(err)
	}
	if err := app.SetFavorite(context.Background(), "alice", "/missing.txt", true); err != nil {
		t.Fatal(err)
	}
	data, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: apps.WidgetStandard}, "files.favorites")
	if err != nil {
		t.Fatal(err)
	}
	if len(data.Items) != 1 || data.Items[0].ID != "/favorite.txt" || !strings.Contains(data.Items[0].URL, "open=") {
		t.Fatalf("items = %#v", data.Items)
	}
}

func TestFavoriteFilesWidgetUsesSemanticSizeLimits(t *testing.T) {
	app := sqliteTestApp(t)
	if err := os.MkdirAll(app.userHome("alice"), 0o700); err != nil {
		t.Fatal(err)
	}
	for i := range 12 {
		name := fmt.Sprintf("favorite-%02d.txt", i)
		path := filepath.Join(app.userHome("alice"), name)
		if err := os.WriteFile(path, []byte(name), 0o600); err != nil {
			t.Fatal(err)
		}
		stamp := time.Unix(int64(1000+i), 0)
		if err := os.Chtimes(path, stamp, stamp); err != nil {
			t.Fatal(err)
		}
		if err := app.SetFavorite(context.Background(), "alice", "/"+name, true); err != nil {
			t.Fatal(err)
		}
	}
	for size, want := range map[apps.WidgetSize]int{
		apps.WidgetCompact:  1,
		apps.WidgetStandard: 3,
		apps.WidgetWide:     6,
		apps.WidgetLarge:    10,
	} {
		data, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: size}, "files.favorites")
		if err != nil {
			t.Fatal(err)
		}
		if len(data.Items) != want {
			t.Fatalf("%s items = %d, want %d", size, len(data.Items), want)
		}
	}
}
