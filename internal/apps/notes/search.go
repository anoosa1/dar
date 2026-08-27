package notes

import (
	"context"
	"fmt"
	"log"

	"github.com/anoosa/dar/internal/apps"
	searchindex "github.com/anoosa/dar/internal/search"
)

func (a *App) IndexSearch(ctx context.Context, uid string, sink apps.SearchDocumentSink) error {
	notes, err := a.indexedNotes(ctx, uid, false)
	if err != nil {
		return err
	}
	for _, note := range notes {
		for _, doc := range noteSearchDocuments(note) {
			if err := sink.AddSearchDocument(ctx, doc); err != nil {
				return err
			}
		}
	}
	return nil
}

func noteSearchDocuments(note Note) []apps.SearchDocument {
	objectID := fmt.Sprint(note.ID)
	chunks := searchindex.ChunkText(note.Content)
	docs := make([]apps.SearchDocument, 0, len(chunks))
	for i, chunk := range chunks {
		docs = append(docs, apps.SearchDocument{
			ObjectID: objectID, ChunkID: fmt.Sprintf("%d", i), Title: note.Title,
			Subtitle: note.Category, Keywords: note.Category, Body: chunk,
			URL: "/notes/note/" + objectID, Anchor: "editor-shell", ModifiedAt: note.Modified,
		})
	}
	return docs
}

func (a *App) replaceNoteSearch(ctx context.Context, uid string, note Note) {
	if a.search == nil {
		return
	}
	if err := a.search.ReplaceSearchObject(ctx, uid, fmt.Sprint(note.ID), noteSearchDocuments(note)); err != nil {
		log.Printf("notes: update search index: %v", err)
		a.markSearchDirty()
	}
}

func (a *App) deleteNoteSearch(ctx context.Context, uid string, id int64) {
	if a.search == nil {
		return
	}
	if err := a.search.DeleteSearchObject(ctx, uid, fmt.Sprint(id)); err != nil {
		log.Printf("notes: delete search index: %v", err)
		a.markSearchDirty()
	}
}

func (a *App) markSearchDirty() {
	if a.search != nil {
		a.search.MarkSearchDirty("notes")
	}
}
