package notifications

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/server"
	"github.com/go-chi/chi/v5"
)

func writeJSON(w http.ResponseWriter, status int, value any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(value)
}

func writeError(w http.ResponseWriter, status int, message string) {
	writeJSON(w, status, map[string]string{"error": message})
}

func decodeStrict(w http.ResponseWriter, r *http.Request, dst any) error {
	r.Body = http.MaxBytesReader(w, r.Body, 16<<10)
	decoder := json.NewDecoder(r.Body)
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(dst); err != nil {
		return err
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		if err == nil {
			return errors.New("request must contain one JSON value")
		}
		return err
	}
	return nil
}

// HandleList returns the current user's undismissed notifications.
func (s *Service) HandleList(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	limit := 50
	if raw := r.URL.Query().Get("limit"); raw != "" {
		value, err := strconv.Atoi(raw)
		if err != nil || value < 1 || value > 100 {
			writeError(w, http.StatusBadRequest, "limit must be between 1 and 100")
			return
		}
		limit = value
	}
	items, count, err := s.list(r.Context(), user.ID, limit)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not load notifications")
		return
	}
	prefs, err := s.loadPreferences(r.Context(), user.ID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not load notification preferences")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"notifications":   items,
		"count":           count,
		"desktop_enabled": prefs.DesktopEnabled,
	})
}

// HandleDismiss dismisses one notification owned by the current user.
func (s *Service) HandleDismiss(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	id := strings.TrimSpace(chi.URLParam(r, "id"))
	if id == "" {
		writeError(w, http.StatusBadRequest, "notification id is required")
		return
	}
	changed, err := s.dismiss(r.Context(), user.ID, id)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not dismiss notification")
		return
	}
	if !changed {
		writeError(w, http.StatusNotFound, "notification not found")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// HandleDismissAll dismisses every active notification for the current user.
func (s *Service) HandleDismissAll(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	if err := s.dismissAll(r.Context(), user.ID); err != nil {
		writeError(w, http.StatusInternalServerError, "could not dismiss notifications")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// HandlePreferences reads or updates the user's desktop-alert preference.
func (s *Service) HandlePreferences(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	switch r.Method {
	case http.MethodGet:
		value, err := s.loadPreferences(r.Context(), user.ID)
		if err != nil {
			writeError(w, http.StatusInternalServerError, "could not load notification preferences")
			return
		}
		writeJSON(w, http.StatusOK, value)
	case http.MethodPut:
		var value notificationPreferences
		if err := decodeStrict(w, r, &value); err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		if err := s.savePreferences(r.Context(), user.ID, value); err != nil {
			writeError(w, http.StatusInternalServerError, "could not save notification preferences")
			return
		}
		writeJSON(w, http.StatusOK, value)
	default:
		w.Header().Set("Allow", "GET, PUT")
		writeError(w, http.StatusMethodNotAllowed, "method not allowed")
	}
}

// HandleStream streams notification changes to one signed-in browser session.
func (s *Service) HandleStream(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	flusher, ok := w.(http.Flusher)
	if !ok {
		writeError(w, http.StatusInternalServerError, "streaming is unavailable")
		return
	}
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("X-Accel-Buffering", "no")

	events, unsubscribe := s.subscribe(user.ID)
	defer unsubscribe()

	writeSSE(w, "notification.resync", map[string]bool{"ok": true})
	flusher.Flush()
	heartbeat := time.NewTicker(25 * time.Second)
	defer heartbeat.Stop()

	for {
		select {
		case <-s.closed:
			return
		case <-r.Context().Done():
			return
		case event := <-events:
			writeSSE(w, event.name, event.data)
			flusher.Flush()
		case <-heartbeat.C:
			// A resync heartbeat makes dropped fan-out events self-healing.
			writeSSE(w, "notification.resync", map[string]bool{"ok": true})
			flusher.Flush()
		}
	}
}

func writeSSE(w io.Writer, name string, value any) {
	_, _ = fmt.Fprintf(w, "event: %s\ndata: %s\n\n", safeEventName(name), encodeEventData(value))
}
