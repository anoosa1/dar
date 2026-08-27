package apps

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strconv"
	"strings"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/server"
	"github.com/go-chi/chi/v5"
)

// InvokeAssistantHTTP adapts an app's existing, user-scoped API boundary into
// an AssistantProvider result. It is intended as a transition helper while
// apps move handler logic into shared backend methods; authentication is
// injected from the trusted userID and never accepted from tool arguments.
func InvokeAssistantHTTP(ctx context.Context, userID string, handler http.HandlerFunc, method, target string, arguments json.RawMessage) (AssistantResult, error) {
	return InvokeAssistantHTTPParams(ctx, userID, handler, method, target, arguments, nil)
}

// InvokeAssistantHTTPParams is InvokeAssistantHTTP with trusted router path
// parameters supplied by the provider rather than by model-controlled routing.
func InvokeAssistantHTTPParams(ctx context.Context, userID string, handler http.HandlerFunc, method, target string, arguments json.RawMessage, routeParams map[string]string) (AssistantResult, error) {
	if userID == "" {
		return AssistantResult{}, fmt.Errorf("authenticated user required")
	}
	var body io.Reader
	if method == http.MethodGet || method == http.MethodDelete {
		var values map[string]any
		if len(arguments) > 0 && string(arguments) != "null" {
			if err := json.Unmarshal(arguments, &values); err != nil {
				return AssistantResult{}, fmt.Errorf("invalid arguments: %w", err)
			}
		}
		q := url.Values{}
		for key, value := range values {
			switch v := value.(type) {
			case string:
				q.Set(key, v)
			case bool:
				q.Set(key, strconv.FormatBool(v))
			case float64:
				q.Set(key, strconv.FormatFloat(v, 'f', -1, 64))
			case []any:
				for _, item := range v {
					q.Add(key, fmt.Sprint(item))
				}
			}
		}
		separator := "?"
		if strings.Contains(target, "?") {
			separator = "&"
		}
		if encoded := q.Encode(); encoded != "" {
			target += separator + encoded
		}
	} else {
		body = bytes.NewReader(arguments)
	}
	req := httptest.NewRequest(method, target, body)
	requestContext := context.WithValue(ctx, server.UserContextKey, &auth.User{ID: userID})
	if len(routeParams) > 0 {
		routeContext := chi.NewRouteContext()
		for key, value := range routeParams {
			routeContext.URLParams.Add(key, value)
		}
		requestContext = context.WithValue(requestContext, chi.RouteCtxKey, routeContext)
	}
	req = req.WithContext(requestContext)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	recorder := httptest.NewRecorder()
	handler(recorder, req)
	response := recorder.Result()
	defer response.Body.Close()
	data, err := io.ReadAll(io.LimitReader(response.Body, 256*1024+1))
	if err != nil {
		return AssistantResult{}, err
	}
	if len(data) > 256*1024 {
		data = append(data[:256*1024], []byte("\n[truncated]")...)
	}
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		message := strings.TrimSpace(string(data))
		if message == "" {
			message = response.Status
		}
		category := AssistantInternal
		switch response.StatusCode {
		case http.StatusBadRequest, http.StatusUnprocessableEntity:
			category = AssistantInvalid
		case http.StatusUnauthorized, http.StatusForbidden:
			category = AssistantForbidden
		case http.StatusNotFound:
			category = AssistantNotFound
		case http.StatusConflict, http.StatusPreconditionFailed:
			category = AssistantConflict
		case http.StatusServiceUnavailable, http.StatusBadGateway, http.StatusGatewayTimeout:
			category = AssistantUnavailable
		}
		return AssistantResult{}, &AssistantError{Category: category, Message: message}
	}
	contentType := response.Header.Get("Content-Type")
	if strings.HasPrefix(contentType, "image/") {
		return AssistantResult{Content: []AssistantContent{{Type: "image", ImageURL: "data:" + strings.Split(contentType, ";")[0] + ";base64," + base64.StdEncoding.EncodeToString(data)}}}, nil
	}
	text := strings.TrimSpace(string(data))
	if text == "" {
		text = `{"ok":true}`
	}
	return AssistantResult{Content: []AssistantContent{{Type: "text", Text: text}}}, nil
}
