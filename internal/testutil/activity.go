package testutil

import (
	"context"

	"github.com/anoosa/dar/internal/apps"
)

// ActivitySink is a registry-compatible recorder for application tests.
type ActivitySink struct {
	Events []apps.ActivityEvent
	Err    error
}

func (s *ActivitySink) Name() string          { return "activity" }
func (s *ActivitySink) DisplayName() string   { return "Activity" }
func (s *ActivitySink) Icon() string          { return "" }
func (s *ActivitySink) Description() string   { return "" }
func (s *ActivitySink) AppBarOrder() int      { return -1 }
func (s *ActivitySink) Routes() []apps.Route  { return nil }
func (s *ActivitySink) HomeCardsHTML() string { return "" }
func (s *ActivitySink) EnqueueActivity(_ context.Context, event apps.ActivityEvent) error {
	s.Events = append(s.Events, event)
	return s.Err
}

// RegistryWithActivity registers sink and each supplied app in a fresh registry.
func RegistryWithActivity(sink *ActivitySink, registered ...apps.App) *apps.Registry {
	registry := apps.NewRegistry()
	registry.Register(sink)
	for _, app := range registered {
		registry.Register(app)
	}
	return registry
}
