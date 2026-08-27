package notes

import (
	"context"
	"fmt"
	"testing"

	"github.com/anoosa/dar/internal/apps"
)

func TestNotesWidgetUsesSemanticSizeLimits(t *testing.T) {
	app, _ := testNotesApp(t)
	ctx := context.Background()
	for i := range 12 {
		title := fmt.Sprintf("Note %02d", i)
		content := "Body " + title
		if _, err := app.create(ctx, "alice", notePatch{Title: &title, Content: &content}); err != nil {
			t.Fatal(err)
		}
	}
	for size, want := range map[apps.WidgetSize]int{
		apps.WidgetCompact:  1,
		apps.WidgetStandard: 3,
		apps.WidgetWide:     6,
		apps.WidgetLarge:    10,
	} {
		data, err := app.DashboardWidget(ctx, apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: size}, "notes.list")
		if err != nil {
			t.Fatal(err)
		}
		if len(data.Items) != want {
			t.Fatalf("%s items = %d, want %d", size, len(data.Items), want)
		}
	}
}
