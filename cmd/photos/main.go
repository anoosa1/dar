// Photos daemon — thumbnail generation, album management, REST API.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"image"
	"image/jpeg"
	"io"
	"log"
	"mime"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

)

var (
	port      = flag.Int("port", 8089, "HTTP port")
	dataRoot  = flag.String("data-root", "data", "Root directory for user data")
	thumbSize = flag.Int("thumb-size", 400, "Thumbnail max width")
)

func main() {
	flag.Parse()
	os.MkdirAll(*dataRoot, 0755)
	mux := http.NewServeMux()
	mux.HandleFunc("/api/photos", handlePhotos)
	mux.HandleFunc("/api/photo", handlePhoto)
	mux.HandleFunc("/api/upload", handleUpload)
	mux.HandleFunc("/api/albums", handleAlbums)
	mux.HandleFunc("/api/album", handleAlbum)
	mux.HandleFunc("/thumb/", handleThumb)
	mux.HandleFunc("/health", handleHealth)
	addr := fmt.Sprintf(":%d", *port)
	log.Printf("[photos] listening on :%d", *port)
	log.Fatal(http.ListenAndServe(addr, mux))
}

type Photo struct {
	ID        string `json:"id"`
	Name      string `json:"name"`
	AlbumID   string `json:"album_id"`
	MimeType  string `json:"mime_type"`
	Width     int    `json:"width"`
	Height    int    `json:"height"`
	Size      int64  `json:"size"`
	TakenAt   int64  `json:"taken_at"`
	Favorited bool   `json:"favorited"`
}

type Album struct {
	ID     string `json:"id"`
	Name   string `json:"name"`
	Cover  string `json:"cover"`
	Count  int    `json:"count"`
}

func photosDir(uid string) string {
	d := filepath.Join(*dataRoot, uid, "photos")
	os.MkdirAll(d, 0755)
	return d
}

func userID(r *http.Request) string {
	if uid := r.Header.Get("X-User-Id"); uid != "" { return uid }
	if uid := r.URL.Query().Get("user_id"); uid != "" { return uid }
	return "default"
}

func loadPhotos(uid string) []Photo {
	data, _ := os.ReadFile(filepath.Join(photosDir(uid), "photos.json"))
	var p []Photo
	json.Unmarshal(data, &p)
	if p == nil { p = []Photo{} }
	return p
}
func savePhotos(uid string, p []Photo) {
	data, _ := json.Marshal(p)
	os.WriteFile(filepath.Join(photosDir(uid), "photos.json"), data, 0644)
}
func loadAlbums(uid string) []Album {
	data, _ := os.ReadFile(filepath.Join(photosDir(uid), "albums.json"))
	var a []Album
	json.Unmarshal(data, &a)
	if a == nil { a = []Album{{ID: "all", Name: "All Photos"}} }
	return a
}
func saveAlbums(uid string, a []Album) {
	data, _ := json.Marshal(a)
	os.WriteFile(filepath.Join(photosDir(uid), "albums.json"), data, 0644)
}

func handlePhotos(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	albumID := r.URL.Query().Get("album_id")
	photos := loadPhotos(uid)
	var result []Photo
	for _, p := range photos {
		if albumID != "" && albumID != "all" && p.AlbumID != albumID { continue }
		result = append(result, p)
	}
	sort.Slice(result, func(i, j int) bool { return result[i].TakenAt > result[j].TakenAt })
	writeJSON(w, map[string]any{"photos": result})
}

func handlePhoto(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	id := r.URL.Query().Get("id")
	switch r.Method {
	case "GET":
		photos := loadPhotos(uid)
		for _, p := range photos {
			if p.ID == id { writeJSON(w, p); return }
		}
		http.Error(w, "not found", 404)
	case "PUT":
		var req struct { Favorited *bool `json:"favorited"`; AlbumID string `json:"album_id"` }
		json.NewDecoder(r.Body).Decode(&req)
		photos := loadPhotos(uid)
		for i := range photos {
			if photos[i].ID == id {
				if req.Favorited != nil { photos[i].Favorited = *req.Favorited }
				if req.AlbumID != "" { photos[i].AlbumID = req.AlbumID }
				savePhotos(uid, photos)
				writeJSON(w, photos[i])
				return
			}
		}
		http.Error(w, "not found", 404)
	case "DELETE":
		photos := loadPhotos(uid)
		for i, p := range photos {
			if p.ID == id {
				os.Remove(filepath.Join(photosDir(uid), "originals", id))
				os.Remove(filepath.Join(photosDir(uid), "thumbs", id+".jpg"))
				photos = append(photos[:i], photos[i+1:]...)
				savePhotos(uid, photos)
				writeJSON(w, map[string]string{"status": "ok"})
				return
			}
		}
		http.Error(w, "not found", 404)
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleUpload(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost { http.Error(w, "method not allowed", 405); return }
	uid := userID(r)
	r.ParseMultipartForm(128 << 20)
	file, header, err := r.FormFile("file")
	if err != nil { http.Error(w, err.Error(), 400); return }
	defer file.Close()

	id := fmt.Sprintf("%d", time.Now().UnixNano())
	origDir := filepath.Join(photosDir(uid), "originals")
	thumbDir := filepath.Join(photosDir(uid), "thumbs")
	os.MkdirAll(origDir, 0755)
	os.MkdirAll(thumbDir, 0755)

	origPath := filepath.Join(origDir, id)
	dst, _ := os.Create(origPath)
	io.Copy(dst, file)
	dst.Close()

	// Generate thumbnail
	thumbPath := filepath.Join(thumbDir, id+".jpg")
	genThumbnail(origPath, thumbPath, *thumbSize)

	// Read dimensions
	pW, pH := 0, 0
	if f, err := os.Open(origPath); err == nil {
		defer f.Close()
		if img, _, err := image.DecodeConfig(f); err == nil {
			pW, pH = img.Width, img.Height
		}
	}

	mimeType := header.Header.Get("Content-Type")
	if mimeType == "" { mimeType = mime.TypeByExtension(filepath.Ext(header.Filename)) }

	photo := Photo{
		ID: id, Name: header.Filename, MimeType: mimeType,
		Width: pW, Height: pH, Size: header.Size, TakenAt: time.Now().Unix(),
	}
	photos := loadPhotos(uid)
	photos = append(photos, photo)
	savePhotos(uid, photos)

	// Update album counts
	albums := loadAlbums(uid)
	for i := range albums {
		if albums[i].ID == "all" { albums[i].Count = len(photos) }
	}
	saveAlbums(uid, albums)

	writeJSON(w, photo)
}

func genThumbnail(srcPath, dstPath string, maxWidth int) {
	src, err := os.Open(srcPath)
	if err != nil { return }
	defer src.Close()
	img, _, err := image.Decode(src)
	if err != nil { return }
	b := img.Bounds()
	iw, ih := b.Dx(), b.Dy()
	if iw <= maxWidth { return }
	ratio := float64(maxWidth) / float64(iw)
	newH := int(float64(ih) * ratio)
	scaled := image.NewRGBA(image.Rect(0, 0, maxWidth, newH))
	for y := 0; y < newH; y++ {
		for x := 0; x < maxWidth; x++ {
			sx := int(float64(x) / ratio)
			sy := int(float64(y) / ratio)
			scaled.Set(x, y, img.At(sx, sy))
		}
	}
	dst, err := os.Create(dstPath)
	if err != nil { return }
	defer dst.Close()
	jpeg.Encode(dst, scaled, &jpeg.Options{Quality: 80})
}

func handleThumb(w http.ResponseWriter, r *http.Request) {
	// /thumb/<user_id>/<photo_id>
	parts := strings.Split(strings.TrimPrefix(r.URL.Path, "/thumb/"), "/")
	if len(parts) < 2 { http.Error(w, "bad path", 400); return }
	uid, pid := parts[0], parts[1]
	path := filepath.Join(photosDir(uid), "thumbs", pid+".jpg")
	if _, err := os.Stat(path); err != nil {
		// Fall back to original
		origPath := filepath.Join(photosDir(uid), "originals", pid)
		if f, err := os.Open(origPath); err == nil {
			defer f.Close()
			img, format, _ := image.Decode(f)
			if img != nil {
				// Serve a downscaled version on the fly
				w.Header().Set("Content-Type", "image/"+format)
				b := img.Bounds()
				if b.Dx() > *thumbSize {
					ratio := float64(*thumbSize) / float64(b.Dx())
					scaled := image.NewRGBA(image.Rect(0, 0, *thumbSize, int(float64(b.Dy())*ratio)))
					for y := 0; y < scaled.Bounds().Dy(); y++ {
						for x := 0; x < *thumbSize; x++ {
							scaled.Set(x, y, img.At(int(float64(x)/ratio), int(float64(y)/ratio)))
						}
					}
					jpeg.Encode(w, scaled, &jpeg.Options{Quality: 70})
					return
				}
			}
		}
		http.Error(w, "not found", 404)
		return
	}
	http.ServeFile(w, r, path)
}

func handleAlbums(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	albums := loadAlbums(uid)
	photos := loadPhotos(uid)
	for i := range albums {
		count := 0
		for _, p := range photos {
			if albums[i].ID == "all" || p.AlbumID == albums[i].ID { count++ }
		}
		albums[i].Count = count
	}
	writeJSON(w, map[string]any{"albums": albums})
}

func handleAlbum(w http.ResponseWriter, r *http.Request) {
	uid := userID(r)
	switch r.Method {
	case "POST":
		var a Album
		json.NewDecoder(r.Body).Decode(&a)
		a.ID = fmt.Sprintf("%d", time.Now().UnixNano())
		albums := loadAlbums(uid)
		albums = append(albums, a)
		saveAlbums(uid, albums)
		writeJSON(w, a)
	default:
		http.Error(w, "method not allowed", 405)
	}
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]string{"status": "ok"})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}

