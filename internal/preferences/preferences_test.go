package preferences

import (
	"context"
	"os"
	"testing"

	"github.com/anoosa/dar/internal/db"
	"zombiezen.com/go/sqlite/sqlitex"
)

func TestJSONRoundTrip(t *testing.T) {
	path := "/tmp/dar_test_preferences.sqlite"
	_ = os.Remove(path)
	defer os.Remove(path)

	database, err := db.Open(path, Migrations())
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer database.Close()

	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatalf("Conn: %v", err)
	}
	defer release()

	type prefs struct {
		Path string `json:"path"`
		Flag bool   `json:"flag"`
	}
	var missing prefs
	ok, err := GetJSON(conn, "alice", "files", "settings", &missing)
	if err != nil {
		t.Fatalf("GetJSON missing: %v", err)
	}
	if ok {
		t.Fatal("missing preference returned ok")
	}

	want := prefs{Path: "/docs", Flag: true}
	if err := SetJSON(conn, "alice", "files", "settings", want); err != nil {
		t.Fatalf("SetJSON: %v", err)
	}
	var got prefs
	ok, err = GetJSON(conn, "alice", "files", "settings", &got)
	if err != nil {
		t.Fatalf("GetJSON: %v", err)
	}
	if !ok {
		t.Fatal("stored preference was missing")
	}
	if got != want {
		t.Fatalf("got %+v want %+v", got, want)
	}
}

func TestJSONValidationAndMalformedValues(t *testing.T) {
	database, err := db.Open(t.TempDir()+"/preferences.sqlite", Migrations())
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer release()
	for _, fields := range [][3]string{{"", "app", "key"}, {"user", "", "key"}, {"user", "app", ""}} {
		if _, err := GetJSON(conn, fields[0], fields[1], fields[2], new(any)); err == nil {
			t.Errorf("GetJSON%q accepted empty identity", fields)
		}
		if err := SetJSON(conn, fields[0], fields[1], fields[2], true); err == nil {
			t.Errorf("SetJSON%q accepted empty identity", fields)
		}
	}
	if err := SetJSON(conn, "user", "app", "key", make(chan int)); err == nil {
		t.Fatal("unencodable preference was accepted")
	}
	if err := sqlitex.Execute(conn, `INSERT INTO user_preferences(user_id,app,key,value_json) VALUES('user','app','broken','{')`, nil); err != nil {
		t.Fatal(err)
	}
	if _, err := GetJSON(conn, "user", "app", "broken", new(any)); err == nil {
		t.Fatal("malformed stored JSON was accepted")
	}
}
