// Package daemon manages subprocess daemons for Dar apps.
package daemon

import (
	"context"
	"fmt"
	"log"
	"net/http"
	"os/exec"
	"sync"
	"time"
)

// Config describes a daemon to spawn.
type Config struct {
	Name             string
	Command          string
	Args             []string
	Port             int
	HealthPath       string
	StartupTimeoutMs int
}

// Manager manages daemon processes: spawn, health-check, restart, shutdown.
type Manager struct {
	mu      sync.Mutex
	daemons map[string]*daemon
}

type daemon struct {
	config Config
	cmd    *exec.Cmd
	cancel context.CancelFunc
}

// NewManager creates a new daemon manager.
func NewManager() *Manager {
	return &Manager{
		daemons: make(map[string]*daemon),
	}
}

// Spawn starts a daemon process and waits for its health check.
func (m *Manager) Spawn(config Config) error {
	m.mu.Lock()
	if _, exists := m.daemons[config.Name]; exists {
		m.mu.Unlock()
		return fmt.Errorf("daemon %q already running", config.Name)
	}

	ctx, cancel := context.WithCancel(context.Background())
	d := &daemon{
		config: config,
		cancel: cancel,
	}
	m.daemons[config.Name] = d
	m.mu.Unlock()

	// Start the daemon process.
	cmd := exec.CommandContext(ctx, config.Command, config.Args...)
	cmd.Stdout = log.Writer()
	cmd.Stderr = log.Writer()
	if err := cmd.Start(); err != nil {
		m.remove(config.Name)
		return fmt.Errorf("daemon %q start: %w", config.Name, err)
	}
	d.cmd = cmd

	// Health check.
	timeout := time.Duration(config.StartupTimeoutMs) * time.Millisecond
	if timeout == 0 {
		timeout = 10 * time.Second
	}
	if err := m.healthCheck(config.Port, config.HealthPath, timeout); err != nil {
		cancel()
		_ = cmd.Wait()
		m.remove(config.Name)
		return fmt.Errorf("daemon %q health check: %w", config.Name, err)
	}

	// Monitor in background.
	go m.monitor(config.Name, d)

	log.Printf("daemon %q started on port %d (pid %d)", config.Name, config.Port, cmd.Process.Pid)
	return nil
}

// Shutdown gracefully stops all daemons.
func (m *Manager) Shutdown() {
	m.mu.Lock()
	names := make([]string, 0, len(m.daemons))
	for name := range m.daemons {
		names = append(names, name)
	}
	m.mu.Unlock()

	for _, name := range names {
		m.stop(name)
	}
}

func (m *Manager) stop(name string) {
	m.mu.Lock()
	d, exists := m.daemons[name]
	if !exists {
		m.mu.Unlock()
		return
	}
	delete(m.daemons, name)
	m.mu.Unlock()

	d.cancel()
	if d.cmd != nil && d.cmd.Process != nil {
		_ = d.cmd.Wait()
	}
	log.Printf("daemon %q stopped", name)
}

func (m *Manager) remove(name string) {
	m.mu.Lock()
	delete(m.daemons, name)
	m.mu.Unlock()
}

func (m *Manager) healthCheck(port int, healthPath string, timeout time.Duration) error {
	url := fmt.Sprintf("http://localhost:%d%s", port, healthPath)
	client := &http.Client{Timeout: 2 * time.Second}

	deadline := time.After(timeout)
	ticker := time.NewTicker(200 * time.Millisecond)
	defer ticker.Stop()

	for {
		select {
		case <-deadline:
			return fmt.Errorf("health check timed out after %v", timeout)
		case <-ticker.C:
			resp, err := client.Get(url)
			if err == nil && resp.StatusCode == http.StatusOK {
				resp.Body.Close()
				return nil
			}
			if resp != nil {
				resp.Body.Close()
			}
		}
	}
}

func (m *Manager) monitor(name string, d *daemon) {
	err := d.cmd.Wait()
	if err != nil {
		log.Printf("daemon %q exited: %v", name, err)
	}
	m.remove(name)
}
