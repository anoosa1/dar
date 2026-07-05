// Package soosa provides the AI assistant daemon core: LLM client, tool registry, SSE streaming.
package soosa

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"strings"
	"sync"
	"time"
)

// Daemon runs the Soosa HTTP server.
type Daemon struct {
	Port   int
	Tools  *ToolRegistry
	LLM    *LLMClient
	server *http.Server
}

// ChatRequest is the incoming chat message.
type ChatRequest struct {
	Messages []Message `json:"messages"`
	UserID   string    `json:"user_id,omitempty"`
}

// Message represents a chat message.
type Message struct {
	Role    string `json:"role"` // "user", "assistant", "system"
	Content string `json:"content"`
}

// ChatDelta is a single SSE delta event.
type ChatDelta struct {
	Content string `json:"content,omitempty"`
	Done    bool   `json:"done"`
	Tool    *ToolCall `json:"tool,omitempty"`
}

// ToolCall represents an LLM tool/function call.
type ToolCall struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
	Result    string `json:"result,omitempty"`
	Error     string `json:"error,omitempty"`
}

// NewDaemon creates a new Soosa daemon.
func NewDaemon(port int, llmConfig LLMConfig) *Daemon {
	reg := NewToolRegistry()
	llm := NewLLMClient(llmConfig, reg)
	return &Daemon{
		Port:  port,
		Tools: reg,
		LLM:   llm,
	}
}

// Start starts the daemon HTTP server.
func (d *Daemon) Start() error {
	mux := http.NewServeMux()
	mux.HandleFunc("/chat", d.handleChat)
	mux.HandleFunc("/tools", d.handleTools)
	mux.HandleFunc("/health", d.handleHealth)

	addr := fmt.Sprintf(":%d", d.Port)
	d.server = &http.Server{Addr: addr, Handler: mux}
	log.Printf("[soosa] listening on :%d", d.Port)
	return d.server.ListenAndServe()
}

// Shutdown gracefully stops the daemon.
func (d *Daemon) Shutdown(ctx context.Context) error {
	return d.server.Shutdown(ctx)
}

func (d *Daemon) handleChat(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	var req ChatRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad request: "+err.Error(), http.StatusBadRequest)
		return
	}
	if len(req.Messages) == 0 {
		http.Error(w, "no messages", http.StatusBadRequest)
		return
	}

	// SSE setup
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")
	flusher, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "SSE not supported", http.StatusInternalServerError)
		return
	}

	ctx := r.Context()
	stream := d.LLM.Chat(ctx, req.Messages)

	for delta := range stream {
		data, _ := json.Marshal(delta)
		fmt.Fprintf(w, "data: %s\n\n", data)
		flusher.Flush()
		if delta.Done {
			return
		}
	}
}

func (d *Daemon) handleTools(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		writeJSON(w, d.Tools.List())
	case http.MethodPost:
		var t ToolDef
		if err := json.NewDecoder(r.Body).Decode(&t); err != nil {
			http.Error(w, "bad request: "+err.Error(), http.StatusBadRequest)
			return
		}
		d.Tools.Register(t)
		writeJSON(w, map[string]string{"status": "registered", "name": t.Name})
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func (d *Daemon) handleHealth(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, map[string]string{"status": "ok"})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}

// ── Tool Registry ──

// ToolDef describes a tool that an app registers with Soosa.
type ToolDef struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	Endpoint    string `json:"endpoint"` // HTTP endpoint to call
}

// ToolRegistry manages registered tools.
type ToolRegistry struct {
	mu    sync.RWMutex
	tools map[string]ToolDef
}

// NewToolRegistry creates a new tool registry.
func NewToolRegistry() *ToolRegistry {
	return &ToolRegistry{tools: make(map[string]ToolDef)}
}

// Register adds a tool to the registry.
func (tr *ToolRegistry) Register(t ToolDef) {
	tr.mu.Lock()
	defer tr.mu.Unlock()
	tr.tools[t.Name] = t
}

// List returns all registered tools.
func (tr *ToolRegistry) List() []ToolDef {
	tr.mu.RLock()
	defer tr.mu.RUnlock()
	result := make([]ToolDef, 0, len(tr.tools))
	for _, t := range tr.tools {
		result = append(result, t)
	}
	return result
}

// Get returns a tool by name.
func (tr *ToolRegistry) Get(name string) (ToolDef, bool) {
	tr.mu.RLock()
	defer tr.mu.RUnlock()
	t, ok := tr.tools[name]
	return t, ok
}

// Execute calls a tool's endpoint and returns the result.
func (tr *ToolRegistry) Execute(ctx context.Context, name string, args map[string]any) (string, error) {
	t, ok := tr.Get(name)
	if !ok {
		return "", fmt.Errorf("tool not found: %s", name)
	}

	body, _ := json.Marshal(args)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, t.Endpoint, strings.NewReader(string(body)))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/json")

	client := &http.Client{Timeout: 30 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	result, _ := io.ReadAll(io.LimitReader(resp.Body, 64*1024))
	return string(result), nil
}

// ── LLM Client ──

// LLMConfig holds LLM provider configuration.
type LLMConfig struct {
	APIURL string // OpenAI-compatible API URL
	APIKey string
	Model  string
}

// LLMClient handles chat completions with an OpenAI-compatible API.
type LLMClient struct {
	config   LLMConfig
	tools    *ToolRegistry
	client   *http.Client
}

// NewLLMClient creates a new LLM client.
func NewLLMClient(cfg LLMConfig, tools *ToolRegistry) *LLMClient {
	return &LLMClient{
		config: cfg,
		tools:  tools,
		client: &http.Client{Timeout: 120 * time.Second},
	}
}

// Chat sends messages to the LLM and returns a channel of deltas for SSE streaming.
func (c *LLMClient) Chat(ctx context.Context, messages []Message) <-chan ChatDelta {
	ch := make(chan ChatDelta, 32)

	go func() {
		defer close(ch)

		// If no LLM configured, return a mock response.
		if c.config.APIURL == "" || c.config.APIKey == "" {
			ch <- ChatDelta{Content: "Soosa AI is not configured. Set SOOSA_API_URL, SOOSA_API_KEY, and SOOSA_MODEL environment variables.\n\nRegistered tools: " + c.toolList()}
			ch <- ChatDelta{Done: true}
			return
		}

		// Build the API request.
		reqBody := map[string]any{
			"model":    c.config.Model,
			"messages": messages,
			"stream":   true,
		}

		// Add tool definitions if any are registered.
		tools := c.tools.List()
		if len(tools) > 0 {
			reqBody["tools"] = c.toolDefs(tools)
		}

		body, _ := json.Marshal(reqBody)
		req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.config.APIURL, strings.NewReader(string(body)))
		if err != nil {
			ch <- ChatDelta{Content: fmt.Sprintf("Error: %v", err)}
			ch <- ChatDelta{Done: true}
			return
		}
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("Authorization", "Bearer "+c.config.APIKey)

		resp, err := c.client.Do(req)
		if err != nil {
			ch <- ChatDelta{Content: fmt.Sprintf("Error: %v", err)}
			ch <- ChatDelta{Done: true}
			return
		}
		defer resp.Body.Close()

		if resp.StatusCode != http.StatusOK {
			body, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
			ch <- ChatDelta{Content: fmt.Sprintf("API error (%d): %s", resp.StatusCode, string(body))}
			ch <- ChatDelta{Done: true}
			return
		}

		// Parse SSE stream from the LLM provider.
		scanner := bufio.NewScanner(resp.Body)
		for scanner.Scan() {
			line := scanner.Text()
			if !strings.HasPrefix(line, "data: ") {
				continue
			}
			data := strings.TrimPrefix(line, "data: ")
			if data == "[DONE]" {
				ch <- ChatDelta{Done: true}
				return
			}

			var chunk struct {
				Choices []struct {
					Delta struct {
						Content string `json:"content"`
					} `json:"delta"`
				} `json:"choices"`
			}
			if err := json.Unmarshal([]byte(data), &chunk); err != nil {
				continue
			}
			if len(chunk.Choices) > 0 && chunk.Choices[0].Delta.Content != "" {
				ch <- ChatDelta{Content: chunk.Choices[0].Delta.Content}
			}
		}
		ch <- ChatDelta{Done: true}
	}()

	return ch
}

func (c *LLMClient) toolList() string {
	tools := c.tools.List()
	if len(tools) == 0 {
		return "none"
	}
	var names []string
	for _, t := range tools {
		names = append(names, t.Name)
	}
	return strings.Join(names, ", ")
}

func (c *LLMClient) toolDefs(tools []ToolDef) []map[string]any {
	defs := make([]map[string]any, 0, len(tools))
	for _, t := range tools {
		defs = append(defs, map[string]any{
			"type": "function",
			"function": map[string]any{
				"name":        t.Name,
				"description": t.Description,
				"parameters": map[string]any{
					"type":       "object",
					"properties": map[string]any{},
				},
			},
		})
	}
	return defs
}
