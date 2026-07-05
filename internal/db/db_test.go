package db

import (
	"context"
	"os"
	"testing"
)

func TestOpenClose(t *testing.T) {
	path := "/tmp/dar_test_open.sqlite"
	_ = os.Remove(path)
	defer os.Remove(path)

	database, err := Open(path, []string{
		"CREATE TABLE IF NOT EXISTS test (id INTEGER PRIMARY KEY, name TEXT);",
	})
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer database.Close()

	ctx := context.Background()
	if err := database.Exec(ctx, "INSERT INTO test (name) VALUES ('hello');"); err != nil {
		t.Fatalf("Exec: %v", err)
	}
}

func TestOpenNoMigrations(t *testing.T) {
	path := "/tmp/dar_test_nomig.sqlite"
	_ = os.Remove(path)
	defer os.Remove(path)

	database, err := Open(path, nil)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	database.Close()
}
