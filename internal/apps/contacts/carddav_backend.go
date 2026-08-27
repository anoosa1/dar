package contacts

import (
	"context"
	"crypto/sha1"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/anoosa/dar/internal/carddav"
	"github.com/anoosa/dar/internal/safefile"
)

// fsCardDAV implements carddav.Backend over the contacts filesystem layout.
type fsCardDAV struct{ app *App }

func (fsCardDAV) PrincipalDisplayName(uid string) string {
	return uid
}

func (b fsCardDAV) ListBooks(uid string) ([]carddav.BookInfo, error) {
	books := b.app.loadAddressBooks(uid)
	out := make([]carddav.BookInfo, 0, len(books))
	for _, book := range books {
		out = append(out, carddav.BookInfo{
			ID:          book.ID,
			DisplayName: book.Name,
			CTag:        b.app.bookCTag(uid, book.ID),
		})
	}
	return out, nil
}

func (b fsCardDAV) BookMeta(uid, bookID string) (carddav.BookInfo, error) {
	bookID = b.app.sanitizeBookID(bookID)
	for _, book := range b.app.loadAddressBooks(uid) {
		if book.ID == bookID {
			return carddav.BookInfo{
				ID:          book.ID,
				DisplayName: book.Name,
				CTag:        b.app.bookCTag(uid, book.ID),
			}, nil
		}
	}
	return carddav.BookInfo{}, carddav.ErrNotFound
}

func (b fsCardDAV) ListCards(uid, bookID string) ([]carddav.CardInfo, error) {
	bookID = b.app.sanitizeBookID(bookID)
	dir := b.app.bookDir(uid, bookID)
	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, err
	}
	var out []carddav.CardInfo
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".vcf") {
			continue
		}
		id := strings.TrimSuffix(e.Name(), ".vcf")
		fi, err := e.Info()
		if err != nil {
			continue
		}
		out = append(out, carddav.CardInfo{
			ID:   id,
			ETag: fileETag(fi.ModTime().Unix(), fi.Size()),
		})
	}
	return out, nil
}

func (b fsCardDAV) GetCard(uid, bookID, id string) ([]byte, string, error) {
	bookID = b.app.sanitizeBookID(bookID)
	id = sanitizeCardID(id)
	if id == "" {
		return nil, "", carddav.ErrNotFound
	}
	path := filepath.Join(b.app.bookDir(uid, bookID), id+".vcf")
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, "", carddav.ErrNotFound
		}
		return nil, "", err
	}
	fi, err := os.Stat(path)
	if err != nil {
		return data, `"0-0"`, nil
	}
	return data, fileETag(fi.ModTime().Unix(), fi.Size()), nil
}

func (b fsCardDAV) PutCard(uid, bookID, id string, vcf []byte, ifMatch, ifNoneMatch string) (string, bool, error) {
	bookID = b.app.sanitizeBookID(bookID)
	id = sanitizeCardID(id)
	if id == "" {
		return "", false, carddav.ErrInvalidCard
	}
	if len(vcf) == 0 {
		return "", false, carddav.ErrInvalidCard
	}
	// Validate parseable vCard.
	if _, err := b.app.decodeCard(vcf); err != nil {
		return "", false, carddav.ErrInvalidCard
	}

	path := filepath.Join(b.app.bookDir(uid, bookID), id+".vcf")
	fi, err := os.Stat(path)
	exists := err == nil

	if ifNoneMatch == "*" && exists {
		return "", false, carddav.ErrPrecondition
	}
	if ifMatch != "" && ifMatch != "*" {
		if !exists {
			return "", false, carddav.ErrPrecondition
		}
		cur := fileETag(fi.ModTime().Unix(), fi.Size())
		if !etagMatch(ifMatch, cur) {
			return "", false, carddav.ErrPrecondition
		}
	} else if ifMatch == "*" && !exists {
		return "", false, carddav.ErrPrecondition
	}

	if err := safefile.Write(path, vcf); err != nil {
		return "", false, err
	}
	b.app.markSearchDirty()
	action := "contact_updated"
	summary := "updated contact " + id
	if !exists {
		action = "contact_created"
		summary = "created contact " + id
	}
	b.app.recordActivity(context.Background(), uid, action, summary, "contact", id, id, "dav")
	fi2, err := os.Stat(path)
	if err != nil {
		return `"0-0"`, !exists, nil
	}
	return fileETag(fi2.ModTime().Unix(), fi2.Size()), !exists, nil
}

func (b fsCardDAV) DeleteCard(uid, bookID, id string) error {
	bookID = b.app.sanitizeBookID(bookID)
	id = sanitizeCardID(id)
	if id == "" {
		return carddav.ErrNotFound
	}
	path := filepath.Join(b.app.bookDir(uid, bookID), id+".vcf")
	err := os.Remove(path)
	if err != nil {
		if os.IsNotExist(err) {
			return carddav.ErrNotFound
		}
		return err
	}
	b.app.markSearchDirty()
	b.app.recordActivity(context.Background(), uid, "contact_deleted", "deleted contact "+id, "contact", id, id, "dav")
	return nil
}

func sanitizeCardID(id string) string {
	id = strings.TrimSpace(id)
	id = strings.TrimSuffix(id, ".vcf")
	if strings.ContainsAny(id, `/\`) {
		return ""
	}
	var b strings.Builder
	for _, r := range id {
		switch {
		case r >= 'a' && r <= 'z', r >= 'A' && r <= 'Z', r >= '0' && r <= '9', r == '-', r == '_', r == '.':
			b.WriteRune(r)
		}
	}
	id = b.String()
	if id == "." || id == ".." {
		return ""
	}
	return id
}

func fileETag(mtimeUnix int64, size int64) string {
	return fmt.Sprintf(`"%d-%d"`, mtimeUnix, size)
}

func etagMatch(header, current string) bool {
	// If-Match can be comma-separated list of etags.
	header = strings.TrimSpace(header)
	current = strings.TrimSpace(current)
	for _, part := range strings.Split(header, ",") {
		p := strings.TrimSpace(part)
		if p == "*" || p == current {
			return true
		}
		// Compare unquoted
		if unquoteETag(p) == unquoteETag(current) {
			return true
		}
	}
	return false
}

func unquoteETag(s string) string {
	s = strings.TrimSpace(s)
	if len(s) >= 2 && s[0] == '"' && s[len(s)-1] == '"' {
		return s[1 : len(s)-1]
	}
	return s
}

func (a *App) bookCTag(uid, bookID string) string {
	dir := a.bookDir(uid, bookID)
	entries, err := os.ReadDir(dir)
	if err != nil {
		return "0"
	}
	h := sha1.New()
	var maxM int64
	var n int
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".vcf") {
			continue
		}
		fi, err := e.Info()
		if err != nil {
			continue
		}
		n++
		if fi.ModTime().Unix() > maxM {
			maxM = fi.ModTime().Unix()
		}
		fmt.Fprintf(h, "%s:%d:%d;", e.Name(), fi.ModTime().Unix(), fi.Size())
	}
	sum := hex.EncodeToString(h.Sum(nil))[:12]
	return fmt.Sprintf("%d-%d-%s", n, maxM, sum)
}
