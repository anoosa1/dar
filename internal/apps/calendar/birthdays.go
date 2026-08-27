package calendar

import (
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/emersion/go-vcard"
)

// birthdayEvents projects Contacts vCards into a read-only calendar. They are
// deliberately never persisted as Calendar events: Contacts remains the owner
// and editing/deleting a contact immediately changes Calendar output.
func (a *App) birthdayEvents(uid string) []Event {
	root := filepath.Join(a.dataRoot, uid, "addressbooks")
	books, _ := os.ReadDir(root)
	var events []Event
	for _, book := range books {
		if !book.IsDir() {
			continue
		}
		files, _ := os.ReadDir(filepath.Join(root, book.Name()))
		for _, file := range files {
			if file.IsDir() || !strings.HasSuffix(file.Name(), ".vcf") {
				continue
			}
			f, err := os.Open(filepath.Join(root, book.Name(), file.Name()))
			if err != nil {
				continue
			}
			card, err := vcard.NewDecoder(f).Decode()
			_ = f.Close()
			if err != nil {
				continue
			}
			bday := strings.TrimSpace(card.Value(vcard.FieldBirthday))
			when, ok := birthdayDate(bday)
			if !ok {
				continue
			}
			name := strings.TrimSpace(card.PreferredValue(vcard.FieldFormattedName))
			if name == "" {
				name = strings.TrimSuffix(file.Name(), ".vcf")
			}
			uidValue := strings.TrimSpace(card.Value(vcard.FieldUID))
			if uidValue == "" {
				uidValue = book.Name() + "-" + file.Name()
			}
			events = append(events, Event{ID: "birthday-" + uidValue, CalendarID: "birthdays", Title: name + "’s birthday", Start: when.Unix(), End: when.AddDate(0, 0, 1).Unix(), AllDay: true, Color: calendarTheme.Flamingo, RRule: "FREQ=YEARLY", Busy: false, Status: "CONFIRMED"})
		}
	}
	return events
}

func birthdayDate(raw string) (time.Time, bool) {
	raw = strings.ReplaceAll(strings.TrimSpace(raw), "-", "")
	if len(raw) != 8 {
		return time.Time{}, false
	}
	t, err := time.ParseInLocation("20060102", raw, time.Local)
	return t, err == nil
}
