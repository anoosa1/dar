package todo

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
)

func TestUpcomingTasksWidgetIncludesOverdueAndExcludesCompleted(t *testing.T) {
	app := testApp(t)
	list, err := app.ensureDefaultList("alice")
	if err != nil {
		t.Fatal(err)
	}
	now := time.Now().Unix()
	for _, task := range []Task{
		{ID: "overdue", ListID: list.ID, Title: "Overdue", Status: "NEEDS-ACTION", DueAt: now - 3600},
		{ID: "soon", ListID: list.ID, Title: "Soon", Status: "IN-PROCESS", DueAt: now + 3600},
		{ID: "done", ListID: list.ID, Title: "Done", Status: "COMPLETED", DueAt: now + 1800},
		{ID: "undated", ListID: list.ID, Title: "Undated", Status: "NEEDS-ACTION"},
	} {
		if err := app.exec(`INSERT INTO todo_tasks(id,list_id,title,status,due_at,created_at,updated_at) VALUES(?,?,?,?,?,?,?)`, task.ID, task.ListID, task.Title, task.Status, task.DueAt, now, now); err != nil {
			t.Fatal(err)
		}
	}
	data, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: apps.WidgetStandard}, "todo.upcoming")
	if err != nil {
		t.Fatal(err)
	}
	if len(data.Items) != 2 || data.Items[0].ID != "overdue" || data.Items[1].ID != "soon" {
		t.Fatalf("items = %#v", data.Items)
	}
}

func TestUpcomingTasksWidgetUsesSemanticLayouts(t *testing.T) {
	app := testApp(t)
	list, err := app.ensureDefaultList("alice")
	if err != nil {
		t.Fatal(err)
	}
	now := time.Now().Unix()
	for i := range 12 {
		due := now + int64(i+1)*3600
		if i < 3 {
			due = now - int64(i+1)*3600
		}
		if err := app.exec(`INSERT INTO todo_tasks(id,list_id,title,status,due_at,created_at,updated_at) VALUES(?,?,?,?,?,?,?)`,
			fmt.Sprintf("task-%02d", i), list.ID, fmt.Sprintf("Task %02d", i), "NEEDS-ACTION", due, now, now); err != nil {
			t.Fatal(err)
		}
	}
	for size, want := range map[apps.WidgetSize]int{
		apps.WidgetCompact:  1,
		apps.WidgetStandard: 3,
		apps.WidgetWide:     6,
	} {
		data, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: size}, "todo.upcoming")
		if err != nil {
			t.Fatal(err)
		}
		if len(data.Items) != want {
			t.Fatalf("%s items = %d, want %d", size, len(data.Items), want)
		}
	}
	large, err := app.DashboardWidget(context.Background(), apps.DashboardWidgetRequest{UserID: "alice", Timezone: "UTC", Size: apps.WidgetLarge}, "todo.upcoming")
	if err != nil {
		t.Fatal(err)
	}
	if large.Kind != "sections" || len(large.Sections) != 2 ||
		len(large.Sections[0].Items) != 3 || len(large.Sections[1].Items) != 7 {
		t.Fatalf("large tasks = %#v", large)
	}
}
