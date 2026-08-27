package codex

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"os/exec"
	"regexp"
	"sync"
	"sync/atomic"
)

type rpcError struct {
	Code    int             `json:"code"`
	Message string          `json:"message"`
	Data    json.RawMessage `json:"data,omitempty"`
}

type rpcCallError struct {
	Code    int
	Message string
	Data    json.RawMessage
}

func (e *rpcCallError) Error() string {
	return fmt.Sprintf("Codex RPC error %d: %s", e.Code, e.Message)
}

type wireMessage struct {
	ID     json.RawMessage `json:"id,omitempty"`
	Method string          `json:"method,omitempty"`
	Params json.RawMessage `json:"params,omitempty"`
	Result json.RawMessage `json:"result,omitempty"`
	Error  *rpcError       `json:"error,omitempty"`
}

type rpcResult struct {
	result json.RawMessage
	err    error
}

type protocolClient struct {
	cmd     *exec.Cmd
	stdin   io.WriteCloser
	nextID  atomic.Uint64
	writeMu sync.Mutex
	mu      sync.Mutex
	pending map[uint64]chan rpcResult
	closed  chan struct{}
	exited  chan struct{}
	closeMu sync.Once
	onEvent func(method string, params json.RawMessage)
	onCall  func(id json.RawMessage, method string, params json.RawMessage)
}

func startProtocolClient(cmd *exec.Cmd, onEvent func(string, json.RawMessage), onCall func(json.RawMessage, string, json.RawMessage)) (*protocolClient, error) {
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return nil, err
	}
	c := &protocolClient{cmd: cmd, stdin: stdin, pending: make(map[uint64]chan rpcResult), closed: make(chan struct{}), exited: make(chan struct{}), onEvent: onEvent, onCall: onCall}
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	go c.readLoop(stdout)
	go c.stderrLoop(stderr)
	go func() {
		err := cmd.Wait()
		close(c.exited)
		if err == nil {
			err = errors.New("Codex app-server exited")
		}
		c.fail(err)
	}()
	return c, nil
}

func (c *protocolClient) call(ctx context.Context, method string, params any, out any) error {
	id := c.nextID.Add(1)
	paramsJSON, err := json.Marshal(params)
	if err != nil {
		return err
	}
	ch := make(chan rpcResult, 1)
	c.mu.Lock()
	select {
	case <-c.closed:
		c.mu.Unlock()
		return errors.New("Codex app-server is not running")
	default:
	}
	c.pending[id] = ch
	c.mu.Unlock()
	if err := c.write(map[string]any{"id": id, "method": method, "params": json.RawMessage(paramsJSON)}); err != nil {
		c.removePending(id)
		return err
	}
	select {
	case response := <-ch:
		if response.err != nil {
			return response.err
		}
		if out == nil || len(response.result) == 0 || string(response.result) == "null" {
			return nil
		}
		if err := json.Unmarshal(response.result, out); err != nil {
			return fmt.Errorf("decode %s response: %w", method, err)
		}
		return nil
	case <-ctx.Done():
		c.removePending(id)
		return ctx.Err()
	case <-c.closed:
		return errors.New("Codex app-server stopped")
	}
}

func (c *protocolClient) notify(method string, params any) error {
	return c.write(map[string]any{"method": method, "params": params})
}

func (c *protocolClient) respond(id json.RawMessage, result any, responseErr *rpcError) error {
	message := map[string]any{"id": id}
	if responseErr != nil {
		message["error"] = responseErr
	} else {
		message["result"] = result
	}
	return c.write(message)
}

func (c *protocolClient) write(message any) error {
	data, err := json.Marshal(message)
	if err != nil {
		return err
	}
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	select {
	case <-c.closed:
		return errors.New("Codex app-server is not running")
	default:
	}
	_, err = c.stdin.Write(append(data, '\n'))
	return err
}

func (c *protocolClient) readLoop(reader io.Reader) {
	scanner := bufio.NewScanner(reader)
	scanner.Buffer(make([]byte, 64*1024), 8*1024*1024)
	for scanner.Scan() {
		var message wireMessage
		if err := json.Unmarshal(scanner.Bytes(), &message); err != nil {
			log.Printf("Codex app-server emitted malformed JSON: %v", err)
			continue
		}
		if len(message.ID) > 0 && message.Method == "" {
			var id uint64
			if err := json.Unmarshal(message.ID, &id); err != nil {
				continue
			}
			c.mu.Lock()
			ch := c.pending[id]
			delete(c.pending, id)
			c.mu.Unlock()
			if ch != nil {
				if message.Error != nil {
					ch <- rpcResult{err: &rpcCallError{
						Code:    message.Error.Code,
						Message: message.Error.Message,
						Data:    append(json.RawMessage(nil), message.Error.Data...),
					}}
				} else {
					ch <- rpcResult{result: message.Result}
				}
			}
			continue
		}
		if len(message.ID) > 0 && message.Method != "" {
			if c.onCall != nil {
				go c.onCall(message.ID, message.Method, message.Params)
			} else {
				_ = c.respond(message.ID, nil, &rpcError{Code: -32601, Message: "unsupported server request"})
			}
			continue
		}
		if message.Method != "" && c.onEvent != nil {
			c.onEvent(message.Method, message.Params)
		}
	}
	if err := scanner.Err(); err != nil {
		c.fail(fmt.Errorf("read Codex app-server: %w", err))
	}
}

func (c *protocolClient) stderrLoop(reader io.Reader) {
	scanner := bufio.NewScanner(reader)
	scanner.Buffer(make([]byte, 16*1024), 256*1024)
	for scanner.Scan() {
		line := redactCredentials(scanner.Text())
		if len(line) > 1000 {
			line = line[:1000] + "…"
		}
		log.Printf("codex app-server: %s", line)
	}
}

var credentialPatterns = []*regexp.Regexp{
	regexp.MustCompile(`\bsk-[A-Za-z0-9_-]{8,}\b`),
	regexp.MustCompile(`(?i)\bBearer\s+[A-Za-z0-9._~-]{8,}`),
}

func redactCredentials(line string) string {
	for _, pattern := range credentialPatterns {
		line = pattern.ReplaceAllString(line, "[REDACTED]")
	}
	return line
}

func (c *protocolClient) removePending(id uint64) {
	c.mu.Lock()
	delete(c.pending, id)
	c.mu.Unlock()
}

func (c *protocolClient) fail(err error) {
	c.closeMu.Do(func() {
		close(c.closed)
		_ = c.stdin.Close()
		c.mu.Lock()
		pending := c.pending
		c.pending = make(map[uint64]chan rpcResult)
		c.mu.Unlock()
		for _, ch := range pending {
			ch <- rpcResult{err: err}
		}
	})
}

func (c *protocolClient) stop() {
	c.fail(errors.New("Codex app-server stopped"))
	if c.cmd.Process != nil {
		_ = c.cmd.Process.Kill()
	}
}

func (c *protocolClient) gracefulStop() {
	c.fail(errors.New("Codex app-server is shutting down"))
	if c.cmd.Process != nil {
		_ = c.cmd.Process.Signal(os.Interrupt)
	}
}
