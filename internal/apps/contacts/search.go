package contacts

import (
	"context"
	"os"
	"path/filepath"
	"strings"

	"github.com/anoosa/dar/internal/apps"
)

func (app *App) IndexSearch(ctx context.Context, uid string, sink apps.SearchDocumentSink) error {
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
			contact.BookID = book.ID
			var fields []string
			fields = append(fields, contact.FirstName, contact.MiddleName, contact.LastName, contact.Prefix, contact.Suffix, contact.Email, contact.Phone, contact.Org, contact.Title, contact.Role, contact.Notes, contact.Birthday)
			for _, value := range contact.Emails {
				fields = append(fields, value.Value)
			}
			for _, value := range contact.Phones {
				fields = append(fields, value.Value)
			}
			for _, address := range contact.Addresses {
				fields = append(fields, address.POBox, address.Extended, address.Street, address.Locality, address.Region, address.PostalCode, address.Country)
			}
			fields = append(fields, contact.Groups...)
			doc := apps.SearchDocument{
				ObjectID: contact.ID, ChunkID: "contact", Title: contact.DisplayName(),
				Subtitle: firstNonBlank(contact.Org, contact.Email, book.Name),
				Keywords: strings.Join(fields, " "), Body: contact.Notes,
				URL: "/contacts/" + contact.ID, ModifiedAt: contact.UpdatedAt,
			}
			if err := sink.AddSearchDocument(ctx, doc); err != nil {
				return err
			}
		}
	}
	return nil
}

func firstNonBlank(values ...string) string {
	for _, value := range values {
		if strings.TrimSpace(value) != "" {
			return value
		}
	}
	return ""
}

func (app *App) markSearchDirty() {
	if app.search != nil {
		app.search.MarkSearchDirty("contacts")
	}
}
