package files

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"
)

func BenchmarkList1000Files(b *testing.B) {
	app := sqliteTestApp(b)
	root := app.realPath("benchmark", "/bulk")
	if err := os.MkdirAll(root, 0700); err != nil {
		b.Fatal(err)
	}
	for i := 0; i < 1000; i++ {
		if err := os.WriteFile(app.realPath("benchmark", fmt.Sprintf("/bulk/file-%04d.txt", i)), []byte("data"), 0600); err != nil {
			b.Fatal(err)
		}
	}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		r := withTestUser(httptest.NewRequest(http.MethodGet, "/api/files/list?path=/bulk", nil), "benchmark")
		app.handleList(httptest.NewRecorder(), r)
	}
}

type zeroReader struct{}

func (zeroReader) Read(p []byte) (int, error) {
	for i := range p {
		p[i] = 0
	}
	return len(p), nil
}

func BenchmarkStreamingUploadMemory(b *testing.B) {
	app := sqliteTestApp(b)
	b.ReportAllocs()
	b.SetBytes(8 << 20)
	for i := 0; i < b.N; i++ {
		source := io.LimitReader(zeroReader{}, 8<<20)
		if err := app.WriteStream(context.Background(), "benchmark", fmt.Sprintf("/upload-%d.bin", i), source); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkMixed100Users(b *testing.B) {
	app := sqliteTestApp(b)
	for i := 0; i < 100; i++ {
		if err := app.WriteFile(context.Background(), fmt.Sprintf("user-%03d", i), "/document.txt", []byte("data")); err != nil {
			b.Fatal(err)
		}
	}
	b.ReportAllocs()
	b.RunParallel(func(pb *testing.PB) {
		i := 0
		for pb.Next() {
			uid := fmt.Sprintf("user-%03d", i%100)
			if _, err := app.Stat(context.Background(), uid, "/document.txt"); err != nil {
				b.Error(err)
			}
			i++
		}
	})
}
