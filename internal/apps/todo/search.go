package todo

import (
	"context"
	"strings"

	"github.com/anoosa/dar/internal/apps"
)

func (a *App) IndexSearch(ctx context.Context, uid string, sink apps.SearchDocumentSink) error {
	lists, err := a.lists(uid, false)
	if err != nil {
		return err
	}
	listNames := map[string]string{}
	for _, list := range lists {
		listNames[list.ID] = list.Name
	}
	tasks, err := a.tasks(uid, "", "", false)
	if err != nil {
		return err
	}
	for _, task := range tasks {
		doc := apps.SearchDocument{
			ObjectID: task.ID, ChunkID: "task", Title: task.Title,
			Subtitle: listNames[task.ListID], Keywords: strings.Join(append(task.Tags, listNames[task.ListID]), " "),
			Body: task.Notes, URL: "/todo", Anchor: "task-" + task.ID, ModifiedAt: task.UpdatedAt,
		}
		if err := sink.AddSearchDocument(ctx, doc); err != nil {
			return err
		}
	}
	return nil
}

func (a *App) markSearchDirty() {
	if a.search != nil {
		a.search.MarkSearchDirty("todo")
	}
}
