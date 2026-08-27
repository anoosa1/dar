// Package safefile writes user content durably with same-directory atomic replacement.
package safefile

import (
	"fmt"
	"os"
	"path/filepath"
)

type syncFile interface {
	Write([]byte) (int, error)
	Sync() error
	Close() error
	Name() string
}

type operations struct {
	mkdirAll   func(string, os.FileMode) error
	createTemp func(string, string) (syncFile, error)
	remove     func(string) error
	chmod      func(string, os.FileMode) error
	rename     func(string, string) error
	open       func(string) (syncFile, error)
}

var systemOperations = operations{
	mkdirAll: os.MkdirAll,
	createTemp: func(dir, pattern string) (syncFile, error) {
		return os.CreateTemp(dir, pattern)
	},
	remove: os.Remove,
	chmod:  os.Chmod,
	rename: os.Rename,
	open: func(path string) (syncFile, error) {
		return os.Open(path)
	},
}

func Write(path string, data []byte) error {
	return write(path, data, systemOperations)
}

func write(path string, data []byte, ops operations) error {
	dir := filepath.Dir(path)
	if err := ops.mkdirAll(dir, 0700); err != nil {
		return err
	}
	tmp, err := ops.createTemp(dir, ".dar-write-*")
	if err != nil {
		return err
	}
	tmpPath := tmp.Name()
	defer ops.remove(tmpPath)
	if _, err = tmp.Write(data); err != nil {
		_ = tmp.Close()
		return err
	}
	if err = tmp.Sync(); err != nil {
		_ = tmp.Close()
		return err
	}
	if err = tmp.Close(); err != nil {
		return err
	}
	if err = ops.chmod(tmpPath, 0600); err != nil {
		return err
	}
	if err = ops.rename(tmpPath, path); err != nil {
		return err
	}
	dirHandle, err := ops.open(dir)
	if err != nil {
		return fmt.Errorf("open parent directory: %w", err)
	}
	err = dirHandle.Sync()
	closeErr := dirHandle.Close()
	if err != nil {
		return fmt.Errorf("sync parent directory: %w", err)
	}
	return closeErr
}
