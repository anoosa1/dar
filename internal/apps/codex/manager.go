package codex

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"
)

const minimumCLIVersion = "0.144.1"

type managerConfig struct {
	Path         string
	HomeRoot     string
	IdleTimeout  time.Duration
	MaxProcesses int
}

type managedProcess struct {
	client    *protocolClient
	home      string
	workspace string
	webSearch string
	loaded    map[string]struct{}
	lastUsed  time.Time
	busy      int
	streams   int
	restart   bool
}

type processManager struct {
	config  managerConfig
	mu      sync.Mutex
	startMu sync.Mutex
	users   map[string]*managedProcess
	stopCh  chan struct{}
	doneCh  chan struct{}
	onEvent func(userID, method string, params json.RawMessage)
	onCall  func(userID string, client *protocolClient, id json.RawMessage, method string, params json.RawMessage)
	onCrash func(userID string)
}

func newProcessManager(config managerConfig) *processManager {
	if config.Path == "" {
		config.Path = "codex"
	}
	if config.IdleTimeout <= 0 {
		config.IdleTimeout = 5 * time.Minute
	}
	return &processManager{config: config, users: make(map[string]*managedProcess), stopCh: make(chan struct{}), doneCh: make(chan struct{})}
}

func (m *processManager) Start(context.Context) error {
	if err := os.MkdirAll(m.config.HomeRoot, 0700); err != nil {
		return fmt.Errorf("create Codex home root: %w", err)
	}
	if err := os.Chmod(m.config.HomeRoot, 0700); err != nil {
		return fmt.Errorf("protect Codex home root: %w", err)
	}
	go m.reaper()
	return nil
}

func (m *processManager) Shutdown(ctx context.Context) error {
	select {
	case <-m.stopCh:
	default:
		close(m.stopCh)
	}
	select {
	case <-m.doneCh:
	case <-ctx.Done():
	}
	m.mu.Lock()
	processes := make([]*managedProcess, 0, len(m.users))
	for _, process := range m.users {
		processes = append(processes, process)
	}
	m.users = make(map[string]*managedProcess)
	m.mu.Unlock()
	for _, process := range processes {
		process.client.gracefulStop()
	}
	for i, process := range processes {
		select {
		case <-process.client.exited:
		case <-ctx.Done():
			for _, remaining := range processes[i:] {
				remaining.client.stop()
			}
			return ctx.Err()
		}
	}
	return nil
}

func (m *processManager) availability(ctx context.Context) (string, error) {
	checkCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	output, err := exec.CommandContext(checkCtx, m.config.Path, "--version").CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("run %s --version: %w", m.config.Path, err)
	}
	version := parseCodexVersion(string(output))
	if version == "" {
		return "", fmt.Errorf("unrecognized Codex version output")
	}
	if compareVersion(version, minimumCLIVersion) < 0 {
		return version, fmt.Errorf("Codex CLI %s or newer is required (found %s)", minimumCLIVersion, version)
	}
	return version, nil
}

func (m *processManager) client(ctx context.Context, userID, webSearch string) (*protocolClient, error) {
	m.mu.Lock()
	if process := m.users[userID]; process != nil {
		select {
		case <-process.client.closed:
			delete(m.users, userID)
		default:
			if process.webSearch == webSearch || process.busy > 0 || process.streams > 0 {
				if process.webSearch != webSearch {
					process.restart = true
				}
				process.lastUsed = time.Now()
				client := process.client
				m.mu.Unlock()
				return client, nil
			}
			process.client.stop()
			delete(m.users, userID)
		}
	}
	m.mu.Unlock()
	m.startMu.Lock()
	defer m.startMu.Unlock()

	// Serialize process creation so the configured capacity cannot be exceeded.
	m.mu.Lock()
	if process := m.users[userID]; process != nil {
		process.lastUsed = time.Now()
		client := process.client
		m.mu.Unlock()
		return client, nil
	}
	var evicted *managedProcess
	if m.config.MaxProcesses > 0 && len(m.users) >= m.config.MaxProcesses {
		var evictUser string
		for uid, process := range m.users {
			if process.busy == 0 && process.streams == 0 && (evicted == nil || process.lastUsed.Before(evicted.lastUsed)) {
				evictUser, evicted = uid, process
			}
		}
		if evicted == nil {
			m.mu.Unlock()
			return nil, errProcessCapacity
		}
		delete(m.users, evictUser)
	}
	m.mu.Unlock()
	if evicted != nil {
		evicted.client.stop()
	}

	if _, err := m.availability(ctx); err != nil {
		return nil, err
	}
	home, workspace, err := m.userDirs(userID)
	if err != nil {
		return nil, err
	}
	args := []string{
		"app-server", "--listen", "stdio://", "--strict-config",
		"--disable", "shell_tool", "--disable", "shell_snapshot",
		"--disable", "apps", "--disable", "plugins", "--disable", "remote_plugin",
		"--disable", "plugin_sharing", "--disable", "hooks", "--disable", "memories",
		"--disable", "multi_agent", "--disable", "goals", "--disable", "image_generation",
		"--disable", "in_app_browser", "--disable", "enable_mcp_apps",
		"-c", "web_search=\"" + webSearch + "\"",
	}
	cmd := exec.Command(m.config.Path, args...)
	cmd.Dir = workspace
	cmd.Env = isolatedCodexEnv(os.Environ(), home)
	client, err := startProtocolClient(cmd, func(method string, params json.RawMessage) {
		m.event(userID, method, params)
	}, func(id json.RawMessage, method string, params json.RawMessage) {
		if m.onCall != nil {
			m.onCall(userID, clientForCallback(m, userID), id, method, params)
		}
	})
	if err != nil {
		return nil, fmt.Errorf("start Codex app-server: %w", err)
	}
	initializeCtx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()
	var initialized struct {
		CodexHome string `json:"codexHome"`
	}
	if err := client.call(initializeCtx, "initialize", map[string]any{"clientInfo": map[string]any{"name": "dar", "title": "Dar", "version": "0.1.0"}, "capabilities": map[string]any{"experimentalApi": true}}, &initialized); err != nil {
		client.stop()
		return nil, fmt.Errorf("initialize Codex app-server: %w", err)
	}
	if filepath.Clean(initialized.CodexHome) != filepath.Clean(home) {
		client.stop()
		return nil, fmt.Errorf("%w: configured %q, received %q", errUnexpectedCodexHome, home, initialized.CodexHome)
	}
	if err := client.notify("initialized", map[string]any{}); err != nil {
		client.stop()
		return nil, err
	}
	m.mu.Lock()
	if existing := m.users[userID]; existing != nil {
		m.mu.Unlock()
		client.stop()
		return existing.client, nil
	}
	m.users[userID] = &managedProcess{client: client, home: home, workspace: workspace, webSearch: webSearch, loaded: make(map[string]struct{}), lastUsed: time.Now()}
	m.mu.Unlock()
	go func() {
		<-client.exited
		m.mu.Lock()
		current := m.users[userID]
		unexpected := current != nil && current.client == client
		if unexpected {
			delete(m.users, userID)
		}
		m.mu.Unlock()
		if unexpected && m.onCrash != nil {
			m.onCrash(userID)
		}
	}()
	return client, nil
}

func (m *processManager) markThreadLoaded(userID string, client *protocolClient, threadID string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if process := m.users[userID]; process != nil && process.client == client {
		process.loaded[threadID] = struct{}{}
		process.lastUsed = time.Now()
	}
}

func (m *processManager) threadLoaded(userID string, client *protocolClient, threadID string) bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	process := m.users[userID]
	if process == nil || process.client != client {
		return false
	}
	_, ok := process.loaded[threadID]
	return ok
}

func (m *processManager) userDirs(userID string) (string, string, error) {
	hash := sha256.Sum256([]byte(userID))
	home := filepath.Join(m.config.HomeRoot, hex.EncodeToString(hash[:16]))
	workspace := filepath.Join(home, "workspace")
	for _, dir := range []string{home, workspace} {
		if err := os.MkdirAll(dir, 0700); err != nil {
			return "", "", err
		}
		if err := os.Chmod(dir, 0700); err != nil {
			return "", "", err
		}
	}
	return home, workspace, nil
}

// clientForCallback resolves the live client without holding it in a closure
// before startProtocolClient has returned.
func clientForCallback(m *processManager, userID string) *protocolClient {
	m.mu.Lock()
	defer m.mu.Unlock()
	if p := m.users[userID]; p != nil {
		return p.client
	}
	return nil
}

func (m *processManager) event(userID, method string, params json.RawMessage) {
	m.mu.Lock()
	if process := m.users[userID]; process != nil {
		process.lastUsed = time.Now()
		switch method {
		case "turn/started":
			process.busy++
		case "turn/completed":
			if process.busy > 0 {
				process.busy--
			}
		case "account/login/completed":
			if process.busy > 0 {
				process.busy--
			}
		}
	}
	m.mu.Unlock()
	if m.onEvent != nil {
		m.onEvent(userID, method, params)
	}
}

func (m *processManager) beginLogin(userID string) {
	m.mu.Lock()
	if p := m.users[userID]; p != nil {
		p.busy++
		p.lastUsed = time.Now()
	}
	m.mu.Unlock()
}

func (m *processManager) finishLogin(userID string) {
	m.mu.Lock()
	if p := m.users[userID]; p != nil && p.busy > 0 {
		p.busy--
	}
	m.mu.Unlock()
}

func (m *processManager) attach(userID string) {
	m.mu.Lock()
	if p := m.users[userID]; p != nil {
		p.streams++
		p.lastUsed = time.Now()
	}
	m.mu.Unlock()
}

func (m *processManager) detach(userID string) {
	m.mu.Lock()
	if p := m.users[userID]; p != nil && p.streams > 0 {
		p.streams--
		p.lastUsed = time.Now()
	}
	m.mu.Unlock()
}

func (m *processManager) restartWhenIdle(userID string) {
	m.mu.Lock()
	if p := m.users[userID]; p != nil {
		p.restart = true
	}
	m.mu.Unlock()
}

func (m *processManager) stopUser(userID string) {
	m.mu.Lock()
	p := m.users[userID]
	delete(m.users, userID)
	m.mu.Unlock()
	if p != nil {
		p.client.stop()
	}
}

func (m *processManager) reaper() {
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	defer close(m.doneCh)
	for {
		select {
		case <-ticker.C:
			now := time.Now()
			var stopped []*managedProcess
			m.mu.Lock()
			for userID, p := range m.users {
				if p.busy == 0 && p.streams == 0 && (p.restart || now.Sub(p.lastUsed) >= m.config.IdleTimeout) {
					delete(m.users, userID)
					stopped = append(stopped, p)
				}
			}
			m.mu.Unlock()
			for _, p := range stopped {
				p.client.stop()
			}
		case <-m.stopCh:
			return
		}
	}
}

func withEnv(env []string, key, value string) []string {
	prefix := key + "="
	out := make([]string, 0, len(env)+1)
	for _, entry := range env {
		if !strings.HasPrefix(entry, prefix) {
			out = append(out, entry)
		}
	}
	return append(out, prefix+value)
}

func isolatedCodexEnv(env []string, home string) []string {
	blocked := map[string]bool{
		"CODEX_HOME": true, "OPENAI_API_KEY": true, "CODEX_API_KEY": true,
		"AZURE_OPENAI_API_KEY": true, "OPENAI_ACCESS_TOKEN": true,
	}
	out := make([]string, 0, len(env)+1)
	for _, entry := range env {
		key := entry
		if index := strings.IndexByte(entry, '='); index >= 0 {
			key = entry[:index]
		}
		if !blocked[key] {
			out = append(out, entry)
		}
	}
	return append(out, "CODEX_HOME="+home)
}

var versionPattern = regexp.MustCompile(`(?i)codex(?:-cli)?\s+v?([0-9]+\.[0-9]+\.[0-9]+)`)

func parseCodexVersion(output string) string {
	match := versionPattern.FindStringSubmatch(output)
	if len(match) == 2 {
		return match[1]
	}
	return ""
}

func compareVersion(a, b string) int {
	parse := func(v string) [3]int {
		var out [3]int
		parts := strings.Split(v, ".")
		for i := 0; i < len(parts) && i < 3; i++ {
			out[i], _ = strconv.Atoi(parts[i])
		}
		return out
	}
	av, bv := parse(a), parse(b)
	for i := range av {
		if av[i] < bv[i] {
			return -1
		}
		if av[i] > bv[i] {
			return 1
		}
	}
	return 0
}

var errProcessUnavailable = errors.New("Codex process unavailable")
var errProcessCapacity = errors.New("all configured Codex process slots are busy")
var errUnexpectedCodexHome = errors.New("Codex app-server used an unexpected home directory")
