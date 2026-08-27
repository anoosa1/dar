// Package db provides a SQLite database wrapper using zombiezen.com/go/sqlite (pure Go, no CGo).
package db

import (
	"context"
	"fmt"
	"os"
	"time"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitemigration"
	"zombiezen.com/go/sqlite/sqlitex"
)

// DB wraps a SQLite connection pool with migration support.
type DB struct {
	pool *sqlitemigration.Pool
}

// Open opens a SQLite database at the given path, runs migrations, and enables WAL mode.
func Open(path string, migrations []string) (*DB, error) {
	if info, err := os.Stat(path); err == nil && info.IsDir() {
		return nil, fmt.Errorf("open database: %s is a directory", path)
	}
	schema := sqlitemigration.Schema{
		Migrations: migrations,
	}
	pool := sqlitemigration.NewPool(path, schema, sqlitemigration.Options{
		Flags:    sqlite.OpenReadWrite | sqlite.OpenCreate | sqlite.OpenWAL | sqlite.OpenURI,
		PoolSize: 5,
		PrepareConn: func(conn *sqlite.Conn) error {
			conn.SetBusyTimeout(5 * time.Second)
			return sqlitex.ExecuteTransient(conn, "PRAGMA foreign_keys = ON;", nil)
		},
	})
	database := &DB{pool: pool}
	// NewPool initializes asynchronously. Take one connection now so migration,
	// WAL, and connection preparation failures abort startup instead of surfacing
	// on the first request.
	startupContext, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	_, release, err := database.Conn(startupContext)
	if err != nil {
		_ = pool.Close()
		return nil, fmt.Errorf("open database: %w", err)
	}
	release()
	return database, nil
}

// Conn gets a connection from the pool. The returned release function MUST be called
// to return the connection to the pool.
func (db *DB) Conn(ctx context.Context) (*sqlite.Conn, func(), error) {
	conn, err := db.pool.Take(ctx)
	if err != nil {
		return nil, nil, fmt.Errorf("db take conn: %w", err)
	}
	return conn, func() { db.pool.Put(conn) }, nil
}

// Exec executes a SQL statement. Use only for DDL or trusted SQL without parameters.
func (db *DB) Exec(ctx context.Context, sql string) error {
	conn, release, err := db.Conn(ctx)
	if err != nil {
		return err
	}
	defer release()
	if err := sqlitex.Execute(conn, sql, &sqlitex.ExecOptions{}); err != nil {
		return fmt.Errorf("db exec: %w", err)
	}
	return nil
}

// ExecNoArgs executes a SQL statement with no args. Alias for Exec.
func (db *DB) ExecNoArgs(ctx context.Context, sql string) error {
	return db.Exec(ctx, sql)
}

// Close closes the database pool.
func (db *DB) Close() error {
	return db.pool.Close()
}
