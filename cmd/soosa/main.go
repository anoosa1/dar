// Soosa AI daemon — LLM chat with SSE streaming and tool registry.
package main

import (
	"flag"
	"log"
	"os"
	"os/signal"
	"syscall"

	"github.com/anoosa/dar/internal/soosa"
)

func main() {
	port := flag.Int("port", 8081, "HTTP port")
	flag.Parse()

	cfg := soosa.LLMConfig{
		APIURL: os.Getenv("SOOSA_API_URL"),
		APIKey: os.Getenv("SOOSA_API_KEY"),
		Model:  os.Getenv("SOOSA_MODEL"),
	}
	if cfg.Model == "" {
		cfg.Model = "gpt-4o-mini"
	}

	if cfg.APIURL == "" {
		log.Printf("[soosa] no SOOSA_API_URL set — running in demo mode (mock responses)")
	}

	d := soosa.NewDaemon(*port, cfg)

	// Graceful shutdown
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		<-sigCh
		log.Printf("[soosa] shutting down...")
		d.Shutdown(nil)
	}()

	if err := d.Start(); err != nil {
		log.Fatalf("[soosa] server error: %v", err)
	}
}
