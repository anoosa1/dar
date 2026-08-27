package codex

import "testing"

func TestEventHubReplayAndResync(t *testing.T) {
	hub := newEventHub()
	for i := 0; i < eventBufferSize+3; i++ {
		hub.publish("delta", map[string]int{"n": i})
	}
	_, replay, resync, unsubscribe := hub.subscribe(2)
	unsubscribe()
	if !resync || len(replay) != 0 {
		t.Fatalf("expected resync, replay=%d resync=%v", len(replay), resync)
	}
	_, replay, resync, unsubscribe = hub.subscribe(uint64(eventBufferSize + 1))
	defer unsubscribe()
	if resync || len(replay) != 2 || replay[0].ID != uint64(eventBufferSize+2) {
		t.Fatalf("replay=%#v resync=%v", replay, resync)
	}
}

func TestEventHubDeliversLiveEvents(t *testing.T) {
	hub := newEventHub()
	channel, _, _, unsubscribe := hub.subscribe(0)
	defer unsubscribe()
	want := hub.publish("completed", map[string]bool{"ok": true})
	if got := <-channel; got.ID != want.ID || got.Type != "completed" {
		t.Fatalf("event = %#v", got)
	}
}
