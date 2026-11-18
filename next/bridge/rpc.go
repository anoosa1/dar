package main

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strconv"
	"sync"
	"sync/atomic"
)

// The bridge speaks newline-delimited JSON with the Node runtime over stdio.
// Node sends requests ({id, method, params}) and the bridge answers ({id, result|error}).
// The bridge sends its own requests for key storage (ids start with "g") and
// one-way events ({event, account, data}). Nothing is ever logged to stdout.

type wireError struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

type frame struct {
	ID      string          `json:"id,omitempty"`
	Method  string          `json:"method,omitempty"`
	Params  json.RawMessage `json:"params,omitempty"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *wireError      `json:"error,omitempty"`
	Event   string          `json:"event,omitempty"`
	Account string          `json:"account,omitempty"`
	Data    any             `json:"data,omitempty"`
}

// codedError carries a stable code the runtime maps to a message for people.
type codedError struct {
	code    string
	message string
}

func (e *codedError) Error() string { return e.message }

func fail(code, message string) error { return &codedError{code, message} }

type handler func(ctx context.Context, params json.RawMessage) (any, error)

type peer struct {
	out      *bufio.Writer
	outLock  sync.Mutex
	next     atomic.Uint64
	pending  sync.Map // id -> chan frame
	handlers map[string]handler
	ctx      context.Context
}

func newPeer(ctx context.Context, w io.Writer) *peer {
	return &peer{out: bufio.NewWriterSize(w, 1<<16), handlers: map[string]handler{}, ctx: ctx}
}

func (p *peer) write(f frame) error {
	line, err := json.Marshal(f)
	if err != nil {
		return err
	}
	p.outLock.Lock()
	defer p.outLock.Unlock()
	if _, err = p.out.Write(append(line, '\n')); err != nil {
		return err
	}
	return p.out.Flush()
}

// emit sends a one-way event. Failures mean the runtime is gone; the read loop ends the process.
func (p *peer) emit(account, event string, data any) {
	_ = p.write(frame{Event: event, Account: account, Data: data})
}

// call sends a request to the runtime and waits for its answer.
func (p *peer) call(ctx context.Context, method string, params any, result any) error {
	raw, err := json.Marshal(params)
	if err != nil {
		return err
	}
	id := "g" + strconv.FormatUint(p.next.Add(1), 10)
	answer := make(chan frame, 1)
	p.pending.Store(id, answer)
	defer p.pending.Delete(id)
	if err = p.write(frame{ID: id, Method: method, Params: raw}); err != nil {
		return err
	}
	select {
	case f := <-answer:
		if f.Error != nil {
			return fmt.Errorf("runtime %s: %s", method, f.Error.Message)
		}
		if result != nil && len(f.Result) > 0 {
			return json.Unmarshal(f.Result, result)
		}
		return nil
	case <-ctx.Done():
		return ctx.Err()
	case <-p.ctx.Done():
		return p.ctx.Err()
	}
}

func (p *peer) handle(method string, h handler) { p.handlers[method] = h }

// serve reads frames until stdin closes. Requests run concurrently.
func (p *peer) serve(r io.Reader) error {
	scanner := bufio.NewScanner(r)
	scanner.Buffer(make([]byte, 1<<16), 64<<20)
	for scanner.Scan() {
		var f frame
		if err := json.Unmarshal(scanner.Bytes(), &f); err != nil {
			return fmt.Errorf("invalid frame: %w", err)
		}
		if f.Method == "" {
			if ch, ok := p.pending.Load(f.ID); ok {
				ch.(chan frame) <- f
			}
			continue
		}
		if f.ID == "" {
			p.dispatch(f) // notifications (microphone audio) keep their order
			continue
		}
		go p.dispatch(f)
	}
	if err := scanner.Err(); err != nil {
		return err
	}
	return io.EOF
}

func (p *peer) dispatch(f frame) {
	h, ok := p.handlers[f.Method]
	if f.ID == "" {
		// A notification ({method, params} without an id) is never answered.
		if ok {
			_, _ = h(p.ctx, f.Params)
		}
		return
	}
	if !ok {
		_ = p.write(frame{ID: f.ID, Error: &wireError{"unknownMethod", "Unknown method " + f.Method}})
		return
	}
	result, err := h(p.ctx, f.Params)
	if err != nil {
		var coded *codedError
		if errors.As(err, &coded) {
			_ = p.write(frame{ID: f.ID, Error: &wireError{coded.code, coded.message}})
		} else {
			_ = p.write(frame{ID: f.ID, Error: &wireError{"bridgeError", err.Error()}})
		}
		return
	}
	raw, err := json.Marshal(result)
	if err != nil {
		_ = p.write(frame{ID: f.ID, Error: &wireError{"bridgeError", err.Error()}})
		return
	}
	_ = p.write(frame{ID: f.ID, Result: raw})
}
