package db

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
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

func TestSettingsCancellationClosedErrorsAndConcurrency(t *testing.T) {
	database, err := Open(filepath.Join(t.TempDir(), "boundaries.sqlite"), []string{`CREATE TABLE counter(value INTEGER NOT NULL);`})
	if err != nil {
		t.Fatal(err)
	}
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	var journal string
	var foreignKeys int64
	if err := sqlitex.Execute(conn, `PRAGMA journal_mode`, &sqlitex.ExecOptions{ResultFunc: func(stmt *sqlite.Stmt) error {
		journal = stmt.ColumnText(0)
		return nil
	}}); err != nil {
		t.Fatal(err)
	}
	if err := sqlitex.Execute(conn, `PRAGMA foreign_keys`, &sqlitex.ExecOptions{ResultFunc: func(stmt *sqlite.Stmt) error {
		foreignKeys = stmt.ColumnInt64(0)
		return nil
	}}); err != nil {
		t.Fatal(err)
	}
	release()
	if journal != "wal" || foreignKeys != 1 {
		t.Fatalf("journal=%q foreign_keys=%d", journal, foreignKeys)
	}
	if err := database.ExecNoArgs(context.Background(), `INSERT INTO counter(value) VALUES(1)`); err != nil {
		t.Fatal(err)
	}
	if err := database.Exec(context.Background(), `invalid SQL`); err == nil {
		t.Fatal("invalid SQL succeeded")
	}

	var held []*sqlite.Conn
	var releases []func()
	for range 5 {
		conn, release, err := database.Conn(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		held = append(held, conn)
		releases = append(releases, release)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, _, err := database.Conn(ctx); err == nil || !errors.Is(err, context.Canceled) {
		t.Fatalf("pool exhaustion error = %v", err)
	}
	for _, release := range releases {
		release()
	}
	_ = held

	var wg sync.WaitGroup
	for range 20 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			if err := database.ExecNoArgs(ctx, `INSERT INTO counter(value) VALUES(1)`); err != nil {
				t.Errorf("concurrent exec: %v", err)
			}
		}()
	}
	wg.Wait()
	if err := database.Close(); err != nil {
		t.Fatal(err)
	}
	if _, _, err := database.Conn(context.Background()); err == nil {
		t.Fatal("closed database returned a connection")
	}
}

func TestOpenRejectsMigrationAndPathFailures(t *testing.T) {
	if _, err := Open(filepath.Join(t.TempDir(), "invalid.sqlite"), []string{"invalid SQL"}); err == nil {
		t.Fatal("invalid migration succeeded")
	}
	directory := t.TempDir()
	if _, err := Open(directory, nil); err == nil {
		t.Fatal("directory opened as a database")
	}
}

func TestForeignKeysEnabledOnEveryConnection(t *testing.T) {
	database, err := Open(filepath.Join(t.TempDir(), "foreign-keys.sqlite"), []string{
		`CREATE TABLE parent(id INTEGER PRIMARY KEY);`,
		`CREATE TABLE child(parent_id INTEGER REFERENCES parent(id));`,
	})
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	for i := 0; i < 5; i++ {
		conn, release, err := database.Conn(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		err = sqlitex.Execute(conn, `INSERT INTO child(parent_id) VALUES(999)`, nil)
		release()
		if err == nil {
			t.Fatalf("connection %d did not enforce foreign keys", i)
		}
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
