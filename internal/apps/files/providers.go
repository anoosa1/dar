package files

import (
	"context"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/anoosa/dar/internal/apps"
)

func (a *App) DashboardWidgets() []apps.WidgetDefinition {
	return []apps.WidgetDefinition{{
		ID: "files.favorites", Title: "Favorite files", Icon: "fa-solid fa-star", Kind: "list",
		DefaultSize: apps.WidgetStandard, SupportedSizes: apps.AllDashboardWidgetSizes(),
		EmptyState: "No favorite files",
		Actions:    []apps.WidgetAction{{ID: "open", Label: "Open Favorites", URL: "/files?filter=favorites", Icon: "fa-solid fa-arrow-up-right-from-square"}},
	}}
}

func (a *App) DashboardWidget(ctx context.Context, request apps.DashboardWidgetRequest, id string) (apps.WidgetData, error) {
	if id != "files.favorites" {
		return apps.WidgetData{}, fmt.Errorf("unknown files widget")
	}
	favorites, err := a.Favorites(ctx, request.UserID)
	if err != nil {
		return apps.WidgetData{}, err
	}
	type favoriteEntry struct {
		path string
		info os.FileInfo
	}
	entries := make([]favoriteEntry, 0, len(favorites))
	for raw, favorite := range favorites {
		path := a.cleanVPath(raw)
		if !favorite || path == "/" || internalFavoritePath(path) {
			continue
		}
		info, statErr := a.Stat(ctx, request.UserID, path)
		if statErr != nil {
			continue
		}
		entries = append(entries, favoriteEntry{path: path, info: info})
	}
	sort.SliceStable(entries, func(i, j int) bool {
		if entries[i].info.ModTime().Equal(entries[j].info.ModTime()) {
			return strings.ToLower(entries[i].path) < strings.ToLower(entries[j].path)
		}
		return entries[i].info.ModTime().After(entries[j].info.ModTime())
	})
	limit := map[apps.WidgetSize]int{
		apps.WidgetCompact: 1, apps.WidgetStandard: 3, apps.WidgetWide: 6, apps.WidgetLarge: 10,
	}[request.Size]
	if len(entries) > limit {
		entries = entries[:limit]
	}
	items := make([]apps.WidgetItem, 0, len(entries))
	for _, entry := range entries {
		itemURL := "/files" + escapeFilePath(entry.path)
		icon := "fa-regular fa-folder"
		if !entry.info.IsDir() {
			itemURL = "/files?open=" + url.QueryEscape(entry.path)
			icon = fileWidgetIcon(filepath.Ext(entry.info.Name()))
		}
		items = append(items, apps.WidgetItem{
			ID: entry.path, Title: entry.info.Name(), Subtitle: entry.path,
			URL: itemURL, Icon: icon, Favorite: true,
		})
	}
	def := a.DashboardWidgets()[0]
	return apps.WidgetData{Kind: def.Kind, Items: items, Actions: def.Actions, EmptyState: def.EmptyState}, nil
}

func internalFavoritePath(path string) bool {
	clean := strings.ToLower("/" + strings.Trim(path, "/") + "/")
	return strings.Contains(clean, "/.versions/") ||
		strings.Contains(clean, "/.trash/") ||
		strings.Contains(clean, "/.local/share/trash/")
}

func fileWidgetIcon(ext string) string {
	switch strings.ToLower(ext) {
	case ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg":
		return "fa-regular fa-image"
	case ".md", ".txt", ".json", ".yaml", ".yml", ".csv":
		return "fa-regular fa-file-lines"
	case ".pdf":
		return "fa-regular fa-file-pdf"
	default:
		return "fa-regular fa-file"
	}
}
