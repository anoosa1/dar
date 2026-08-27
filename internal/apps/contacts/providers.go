package contacts

import (
	"context"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/anoosa/dar/internal/apps"
)

func (app *App) SuggestRecipients(ctx context.Context, uid, query string, limit int) ([]apps.RecipientSuggestion, error) {
	if limit <= 0 || limit > 50 {
		limit = 12
	}
	query = strings.ToLower(strings.TrimSpace(query))
	seen := map[string]bool{}
	var out []apps.RecipientSuggestion
	for _, book := range app.loadAddressBooks(uid) {
		if !book.Visible {
			continue
		}
		entries, _ := os.ReadDir(app.bookDir(uid, book.ID))
		for _, entry := range entries {
			if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".vcf") {
				continue
			}
			contact, err := app.parseVCard(filepath.Join(app.bookDir(uid, book.ID), entry.Name()))
			if err != nil {
				continue
			}
			name := contact.DisplayName()
			emails := contact.Emails
			if len(emails) == 0 && contact.Email != "" {
				emails = []TypedValue{{Value: contact.Email}}
			}
			for _, item := range emails {
				email := strings.TrimSpace(strings.ToLower(item.Value))
				if email == "" || seen[email] {
					continue
				}
				if query != "" && !strings.Contains(strings.ToLower(name+" "+email), query) {
					continue
				}
				seen[email] = true
				out = append(out, apps.RecipientSuggestion{Name: name, Email: email})
			}
		}
	}
	sort.Slice(out, func(i, j int) bool {
		return strings.ToLower(out[i].Name+out[i].Email) < strings.ToLower(out[j].Name+out[j].Email)
	})
	if len(out) > limit {
		out = out[:limit]
	}
	if out == nil {
		out = []apps.RecipientSuggestion{}
	}
	return out, nil
}
