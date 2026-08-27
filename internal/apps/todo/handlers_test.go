package todo

import (
	"bytes"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/webui"
)

func TestTodoPageUsesSidebarPaneAndSettingsModal(t *testing.T) {
	var out bytes.Buffer
	req := httptest.NewRequest("GET", "/todo", nil)
	if err := renderPage(&out, req, "Test User"); err != nil {
		t.Fatal(err)
	}
	html := out.String()
	_, controller := webui.ExternalizeLastScript(todoHTML(), "/todo/assets/app.js")
	contract := html + string(controller)
	for _, want := range []string{
		`id="task-lists"`,
		`id="new-list"`,
		`>New list</span>`,
		`id="todo-settings-modal"`,
		`id="todo-setting-default-list"`,
		`id="todo-smart-settings"`,
		`id="detail-pane"`,
		`id="detail-pane-resizer"`,
		`id="task-editor"`,
		`window.openSettings`,
		`mode==='hidden'||(mode==='automatic'&&count===0)`,
		`DarUI.shell.openDetail($('detail-pane')`,
		`DarUI.shell.closeDetail($('detail-pane')`,
		`api('GET','/lists')`,
	} {
		if !strings.Contains(contract, want) {
			t.Fatalf("Todo HTML does not contain %q", want)
		}
	}
	for _, unwanted := range []string{
		`id="show-editor"`,
		`id="todo-settings-panel"`,
		`My tasks`,
		`id="detail-empty"`,
		`max-w-4xl`,
		`current.trash?'?trash=1'`,
	} {
		if strings.Contains(html, unwanted) {
			t.Fatalf("Todo HTML unexpectedly contains %q", unwanted)
		}
	}
	sortIndex := strings.Index(html, `id="sort-tasks"`)
	quickAddIndex := strings.Index(html, `id="quick-add"`)
	if sortIndex < 0 || quickAddIndex < 0 || sortIndex > quickAddIndex || !strings.Contains(html[sortIndex:quickAddIndex], `</button>`) {
		t.Fatal("icon-only sort control should be immediately before quick add")
	}
	if strings.Contains(html, `id="sort-tasks" type="button" class="h-11 w-11 shrink-0 rounded-full bg-surface-container hover:bg-surface-container-high" aria-label="Sort tasks by priority" title="Sort tasks by priority"><i class="fa-solid fa-arrow-down-wide-short"></i><span`) {
		t.Fatal("sort control should not include visible label text")
	}
}

func TestTodoSidebarCollectionIDsAndSettingsFooter(t *testing.T) {
	var out bytes.Buffer
	req := httptest.NewRequest("GET", "/todo?collection=today", nil)
	if err := renderPage(&out, req, "Test User"); err != nil {
		t.Fatal(err)
	}
	html := out.String()
	for _, name := range []string{"important", "today", "week", "all", "current", "completed"} {
		if !strings.Contains(html, `id="todo-collection-`+name+`"`) {
			t.Fatalf("missing stable smart collection id for %s", name)
		}
	}
	if !strings.Contains(html, `href="#todo-settings" onclick="event.preventDefault();openSettings()"`) {
		t.Fatal("Todo settings footer should open the settings modal")
	}
}
