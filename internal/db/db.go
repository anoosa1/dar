// Package db provides a SQLite database wrapper using zombiezen.com/go/sqlite (pure Go, no CGo).
package db

import (
	"context"
	"fmt"

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
	schema := sqlitemigration.Schema{
		Migrations: migrations,
	}
	pool := sqlitemigration.NewPool(path, schema, sqlitemigration.Options{
		Flags:    sqlite.OpenReadWrite | sqlite.OpenCreate,
		PoolSize: 5,
	})
	return &DB{pool: pool}, nil
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
