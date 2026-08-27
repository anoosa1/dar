package safefile

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
)

type fakeSyncFile struct {
	name              string
	writeErr, syncErr error
	closeErr          error
}

func (f *fakeSyncFile) Write(p []byte) (int, error) { return len(p), f.writeErr }
func (f *fakeSyncFile) Sync() error                 { return f.syncErr }
func (f *fakeSyncFile) Close() error                { return f.closeErr }
func (f *fakeSyncFile) Name() string                { return f.name }

func TestWriteUsesPrivateModesAndAtomicReplacement(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nested", "contact.vcf")
	if err := Write(path, []byte("first")); err != nil {
		t.Fatal(err)
	}
	if err := Write(path, []byte("second")); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(path)
	if err != nil || string(data) != "second" {
		t.Fatalf("content=%q err=%v", data, err)
	}
	fileInfo, _ := os.Stat(path)
	dirInfo, _ := os.Stat(filepath.Dir(path))
	if fileInfo.Mode().Perm() != 0600 || dirInfo.Mode().Perm() != 0700 {
		t.Fatalf("modes file=%o dir=%o", fileInfo.Mode().Perm(), dirInfo.Mode().Perm())
	}
	matches, _ := filepath.Glob(filepath.Join(filepath.Dir(path), ".dar-write-*"))
	if len(matches) != 0 {
		t.Fatalf("temporary files left behind: %v", matches)
	}
}

func TestWritePropagatesEveryFilesystemBoundary(t *testing.T) {
	failure := errors.New("fixture")
	base := operations{
		mkdirAll: func(string, os.FileMode) error { return nil },
		createTemp: func(string, string) (syncFile, error) {
			return &fakeSyncFile{name: "/tmp/fake"}, nil
		},
		remove: func(string) error { return nil },
		chmod:  func(string, os.FileMode) error { return nil },
		rename: func(string, string) error { return nil },
		open: func(string) (syncFile, error) {
			return &fakeSyncFile{name: "/tmp"}, nil
		},
	}
	cases := map[string]func(*operations){
		"mkdir": func(ops *operations) {
			ops.mkdirAll = func(string, os.FileMode) error { return failure }
		},
		"create temp": func(ops *operations) {
			ops.createTemp = func(string, string) (syncFile, error) { return nil, failure }
		},
		"write": func(ops *operations) {
			ops.createTemp = func(string, string) (syncFile, error) {
				return &fakeSyncFile{name: "/tmp/fake", writeErr: failure}, nil
			}
		},
		"sync": func(ops *operations) {
			ops.createTemp = func(string, string) (syncFile, error) { return &fakeSyncFile{name: "/tmp/fake", syncErr: failure}, nil }
		},
		"close": func(ops *operations) {
			ops.createTemp = func(string, string) (syncFile, error) {
				return &fakeSyncFile{name: "/tmp/fake", closeErr: failure}, nil
			}
		},
		"chmod": func(ops *operations) {
			ops.chmod = func(string, os.FileMode) error { return failure }
		},
		"rename": func(ops *operations) {
			ops.rename = func(string, string) error { return failure }
		},
		"open parent": func(ops *operations) {
			ops.open = func(string) (syncFile, error) { return nil, failure }
		},
		"sync parent": func(ops *operations) {
			ops.open = func(string) (syncFile, error) { return &fakeSyncFile{name: "/tmp", syncErr: failure}, nil }
		},
		"close parent": func(ops *operations) {
			ops.open = func(string) (syncFile, error) { return &fakeSyncFile{name: "/tmp", closeErr: failure}, nil }
		},
	}
	for name, mutate := range cases {
		t.Run(name, func(t *testing.T) {
			ops := base
			mutate(&ops)
			if err := write("/tmp/final", []byte("data"), ops); err == nil {
				t.Fatal("filesystem failure was ignored")
			}
		})
	}
}
