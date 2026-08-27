package codex

import (
	"encoding/json"
	"sync"
)

const eventBufferSize = 512

type streamEvent struct {
	ID   uint64          `json:"id"`
	Type string          `json:"type"`
	Data json.RawMessage `json:"data"`
}

type eventHub struct {
	mu          sync.Mutex
	nextID      uint64
	events      []streamEvent
	subscribers map[chan streamEvent]struct{}
}

func newEventHub() *eventHub {
	return &eventHub{subscribers: make(map[chan streamEvent]struct{})}
}

func (h *eventHub) publish(eventType string, value any) streamEvent {
	data, err := json.Marshal(value)
	if err != nil {
		data = []byte(`{"message":"event encoding failed"}`)
	}
	h.mu.Lock()
	h.nextID++
	event := streamEvent{ID: h.nextID, Type: eventType, Data: data}
	h.events = append(h.events, event)
	if len(h.events) > eventBufferSize {
		h.events = append([]streamEvent(nil), h.events[len(h.events)-eventBufferSize:]...)
	}
	for subscriber := range h.subscribers {
		select {
		case subscriber <- event:
		default:
			// A slow browser can reconnect and replay from the bounded buffer.
		}
	}
	h.mu.Unlock()
	return event
}

func (h *eventHub) subscribe(after uint64) (<-chan streamEvent, []streamEvent, bool, func()) {
	h.mu.Lock()
	defer h.mu.Unlock()
	resync := len(h.events) > 0 && after > 0 && after+1 < h.events[0].ID
	var replay []streamEvent
	if !resync {
		for _, event := range h.events {
			if event.ID > after {
				replay = append(replay, event)
			}
		}
	}
	ch := make(chan streamEvent, 32)
	h.subscribers[ch] = struct{}{}
	return ch, replay, resync, func() {
		h.mu.Lock()
		if _, ok := h.subscribers[ch]; ok {
			delete(h.subscribers, ch)
			close(ch)
		}
		h.mu.Unlock()
	}
}

func (h *eventHub) latestID() uint64 {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.nextID
}
