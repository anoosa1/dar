package testutil

import (
	"bufio"
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha1"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

const webSocketGUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

// Browser drives a private headless Chromium process through the DevTools
// protocol without a third-party browser library.
type Browser struct {
	command *exec.Cmd
	socket  *webSocket
	done    chan error

	nextID    atomic.Int64
	pendingMu sync.Mutex
	pending   map[int64]chan cdpMessage

	eventMu sync.Mutex
	events  []cdpMessage
	errors  []string
	logs    []string

	closeOnce sync.Once
}

type cdpMessage struct {
	ID     int64           `json:"id,omitempty"`
	Method string          `json:"method,omitempty"`
	Params json.RawMessage `json:"params,omitempty"`
	Result json.RawMessage `json:"result,omitempty"`
	Error  *struct {
		Code    int    `json:"code"`
		Message string `json:"message"`
	} `json:"error,omitempty"`
}

// LaunchBrowser starts Chromium with an isolated profile and registers cleanup.
func LaunchBrowser(t testing.TB) *Browser {
	t.Helper()
	executable := os.Getenv("DAR_CHROMIUM_PATH")
	if executable == "" {
		for _, candidate := range []string{"chromium", "chromium-browser", "google-chrome", "google-chrome-stable"} {
			if path, err := exec.LookPath(candidate); err == nil {
				executable = path
				break
			}
		}
	}
	if executable == "" {
		t.Fatal("Chromium is required for the browser regression suite; set DAR_CHROMIUM_PATH")
	}

	profile := filepath.Join(t.TempDir(), "chromium-profile")
	ctx, cancel := context.WithCancel(context.Background())
	command := exec.CommandContext(ctx, executable,
		"--headless=new",
		"--no-sandbox",
		"--single-process",
		"--disable-dev-shm-usage",
		"--disable-background-networking",
		"--disable-component-update",
		"--disable-default-apps",
		"--disable-extensions",
		"--disable-features=Translate,OptimizationHints,MediaRouter",
		"--disable-sync",
		"--metrics-recording-only",
		"--no-first-run",
		"--no-default-browser-check",
		"--password-store=basic",
		"--remote-debugging-port=0",
		"--remote-allow-origins=*",
		"--user-data-dir="+profile,
		"about:blank",
	)
	stderr, err := command.StderrPipe()
	if err != nil {
		cancel()
		t.Fatalf("Chromium stderr: %v", err)
	}
	var outputMu sync.Mutex
	var output []string
	endpoint := make(chan string, 1)
	go func() {
		scanner := bufio.NewScanner(stderr)
		for scanner.Scan() {
			line := scanner.Text()
			outputMu.Lock()
			output = append(output, line)
			outputMu.Unlock()
			if marker := strings.Index(line, "DevTools listening on "); marker >= 0 {
				select {
				case endpoint <- strings.TrimSpace(line[marker+len("DevTools listening on "):]):
				default:
				}
			}
		}
	}()
	if err := command.Start(); err != nil {
		cancel()
		t.Fatalf("start Chromium: %v", err)
	}
	processDone := make(chan error, 1)
	go func() { processDone <- command.Wait() }()

	var browserEndpoint string
	select {
	case browserEndpoint = <-endpoint:
	case err := <-processDone:
		cancel()
		outputMu.Lock()
		detail := strings.Join(output, "\n")
		outputMu.Unlock()
		t.Fatalf("Chromium exited before DevTools became ready: %v\n%s", err, detail)
	case <-time.After(10 * time.Second):
		cancel()
		_ = command.Process.Kill()
		outputMu.Lock()
		detail := strings.Join(output, "\n")
		outputMu.Unlock()
		t.Fatalf("Chromium DevTools timeout\n%s", detail)
	}

	pageEndpoint, err := createPageEndpoint(browserEndpoint)
	if err != nil {
		cancel()
		_ = command.Process.Kill()
		t.Fatalf("create Chromium page: %v", err)
	}
	socket, err := dialWebSocket(pageEndpoint)
	if err != nil {
		cancel()
		_ = command.Process.Kill()
		t.Fatalf("connect Chromium DevTools: %v", err)
	}
	browser := &Browser{
		command: command,
		socket:  socket,
		done:    processDone,
		pending: map[int64]chan cdpMessage{},
	}
	go browser.readLoop()
	t.Cleanup(func() {
		browser.Close()
		cancel()
	})
	enableCtx, enableCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer enableCancel()
	for _, domain := range []string{"Page.enable", "Runtime.enable", "Network.enable", "Log.enable", "Accessibility.enable"} {
		if err := browser.Call(enableCtx, domain, map[string]any{}, nil); err != nil {
			t.Fatalf("enable %s: %v", domain, err)
		}
	}
	return browser
}

func createPageEndpoint(browserEndpoint string) (string, error) {
	parsed, err := url.Parse(browserEndpoint)
	if err != nil {
		return "", err
	}
	targetURL := "http://" + parsed.Host + "/json/new?about%3Ablank"
	request, err := http.NewRequest(http.MethodPut, targetURL, nil)
	if err != nil {
		return "", err
	}
	client := &http.Client{Timeout: 5 * time.Second}
	response, err := client.Do(request)
	if err != nil {
		return "", err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(response.Body, 4096))
		return "", fmt.Errorf("create target status %d: %s", response.StatusCode, body)
	}
	var target struct {
		WebSocketURL string `json:"webSocketDebuggerUrl"`
	}
	if err := json.NewDecoder(response.Body).Decode(&target); err != nil {
		return "", err
	}
	if target.WebSocketURL == "" {
		return "", errors.New("target did not include a WebSocket URL")
	}
	return target.WebSocketURL, nil
}

// Call sends one DevTools command and waits for its matching response.
func (browser *Browser) Call(ctx context.Context, method string, params any, result any) error {
	id := browser.nextID.Add(1)
	message, err := json.Marshal(map[string]any{"id": id, "method": method, "params": params})
	if err != nil {
		return err
	}
	responseChannel := make(chan cdpMessage, 1)
	browser.pendingMu.Lock()
	browser.pending[id] = responseChannel
	browser.pendingMu.Unlock()
	if err := browser.socket.writeText(message); err != nil {
		browser.removePending(id)
		return err
	}
	select {
	case response := <-responseChannel:
		if response.Error != nil {
			return fmt.Errorf("DevTools %s: [%d] %s", method, response.Error.Code, response.Error.Message)
		}
		if result != nil && len(response.Result) != 0 {
			if err := json.Unmarshal(response.Result, result); err != nil {
				return fmt.Errorf("decode DevTools %s result: %w", method, err)
			}
		}
		return nil
	case <-ctx.Done():
		browser.removePending(id)
		return fmt.Errorf("DevTools %s: %w", method, ctx.Err())
	}
}

func (browser *Browser) removePending(id int64) {
	browser.pendingMu.Lock()
	delete(browser.pending, id)
	browser.pendingMu.Unlock()
}

func (browser *Browser) readLoop() {
	for {
		data, err := browser.socket.readText()
		if err != nil {
			browser.pendingMu.Lock()
			for id, channel := range browser.pending {
				delete(browser.pending, id)
				channel <- cdpMessage{Error: &struct {
					Code    int    `json:"code"`
					Message string `json:"message"`
				}{Code: -1, Message: err.Error()}}
			}
			browser.pendingMu.Unlock()
			return
		}
		var message cdpMessage
		if json.Unmarshal(data, &message) != nil {
			continue
		}
		if message.ID != 0 {
			browser.pendingMu.Lock()
			channel := browser.pending[message.ID]
			delete(browser.pending, message.ID)
			browser.pendingMu.Unlock()
			if channel != nil {
				channel <- message
			}
			continue
		}
		browser.recordEvent(message)
	}
}

func (browser *Browser) recordEvent(message cdpMessage) {
	browser.eventMu.Lock()
	defer browser.eventMu.Unlock()
	browser.events = append(browser.events, message)
	switch message.Method {
	case "Page.javascriptDialogOpening":
		go func() {
			ctx, cancel := context.WithTimeout(context.Background(), time.Second)
			defer cancel()
			_ = browser.Call(ctx, "Page.handleJavaScriptDialog", map[string]any{"accept": true}, nil)
		}()
	case "Runtime.exceptionThrown":
		browser.errors = append(browser.errors, "uncaught exception: "+string(message.Params))
	case "Log.entryAdded":
		var params struct {
			Entry struct {
				Level string `json:"level"`
				Text  string `json:"text"`
			} `json:"entry"`
		}
		if json.Unmarshal(message.Params, &params) == nil {
			browser.logs = append(browser.logs, params.Entry.Level+": "+params.Entry.Text)
			if params.Entry.Level == "error" {
				browser.errors = append(browser.errors, "console: "+params.Entry.Text)
			}
		}
	case "Runtime.consoleAPICalled":
		var params struct {
			Type string `json:"type"`
			Args []struct {
				Value       any    `json:"value"`
				Description string `json:"description"`
			} `json:"args"`
		}
		if json.Unmarshal(message.Params, &params) == nil {
			var parts []string
			for _, argument := range params.Args {
				if argument.Value != nil {
					parts = append(parts, fmt.Sprint(argument.Value))
				} else {
					parts = append(parts, argument.Description)
				}
			}
			line := params.Type + ": " + strings.Join(parts, " ")
			browser.logs = append(browser.logs, line)
			if params.Type == "error" || params.Type == "assert" {
				browser.errors = append(browser.errors, "console: "+line)
			}
		}
	}
}

// Navigate loads target and waits until document.readyState is complete.
func (browser *Browser) Navigate(ctx context.Context, target string) error {
	var response struct {
		ErrorText string `json:"errorText"`
	}
	if err := browser.Call(ctx, "Page.navigate", map[string]any{"url": target}, &response); err != nil {
		return err
	}
	if response.ErrorText != "" {
		return errors.New(response.ErrorText)
	}
	for {
		var ready bool
		if err := browser.Evaluate(ctx, "document.readyState === 'complete'", &ready); err == nil && ready {
			return nil
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(10 * time.Millisecond):
		}
	}
}

// Evaluate runs JavaScript and decodes its JSON-compatible value.
func (browser *Browser) Evaluate(ctx context.Context, expression string, value any) error {
	var response struct {
		Result struct {
			Value       json.RawMessage `json:"value"`
			Description string          `json:"description"`
		} `json:"result"`
		Exception *json.RawMessage `json:"exceptionDetails"`
	}
	if err := browser.Call(ctx, "Runtime.evaluate", map[string]any{
		"expression":    expression,
		"awaitPromise":  true,
		"returnByValue": true,
		"userGesture":   true,
	}, &response); err != nil {
		return err
	}
	if response.Exception != nil {
		return fmt.Errorf("JavaScript exception: %s", *response.Exception)
	}
	if value != nil && len(response.Result.Value) != 0 {
		if err := json.Unmarshal(response.Result.Value, value); err != nil {
			return fmt.Errorf("decode JavaScript value %s: %w", response.Result.Value, err)
		}
	}
	return nil
}

// Click clicks the first element matching selector.
func (browser *Browser) Click(ctx context.Context, selector string) error {
	encoded, _ := json.Marshal(selector)
	return browser.Evaluate(ctx, fmt.Sprintf(`(() => {
		const element = document.querySelector(%s);
		if (!element) throw new Error("missing element " + %s);
		element.click();
		return true;
	})()`, encoded, encoded), nil)
}

// Fill focuses an editable control, replaces its value, and dispatches input/change.
func (browser *Browser) Fill(ctx context.Context, selector, text string) error {
	encodedSelector, _ := json.Marshal(selector)
	encodedText, _ := json.Marshal(text)
	return browser.Evaluate(ctx, fmt.Sprintf(`(() => {
		const element = document.querySelector(%s);
		if (!element) throw new Error("missing element " + %s);
		element.focus();
		element.value = %s;
		element.dispatchEvent(new Event("input", {bubbles:true}));
		element.dispatchEvent(new Event("change", {bubbles:true}));
		return element.value;
	})()`, encodedSelector, encodedSelector, encodedText), nil)
}

// Upload assigns files to a file input without invoking a native picker.
func (browser *Browser) Upload(ctx context.Context, selector string, paths ...string) error {
	var document struct {
		Root struct {
			NodeID int64 `json:"nodeId"`
		} `json:"root"`
	}
	if err := browser.Call(ctx, "DOM.getDocument", map[string]any{"depth": 1}, &document); err != nil {
		return err
	}
	var query struct {
		NodeID int64 `json:"nodeId"`
	}
	if err := browser.Call(ctx, "DOM.querySelector", map[string]any{"nodeId": document.Root.NodeID, "selector": selector}, &query); err != nil {
		return err
	}
	if query.NodeID == 0 {
		return fmt.Errorf("missing file input %q", selector)
	}
	return browser.Call(ctx, "DOM.setFileInputFiles", map[string]any{"nodeId": query.NodeID, "files": paths}, nil)
}

// HTML returns the current serialized document.
func (browser *Browser) HTML(ctx context.Context) (string, error) {
	var html string
	err := browser.Evaluate(ctx, "document.documentElement.outerHTML", &html)
	return html, err
}

// AccessibilityTree returns Chromium's full accessibility tree.
func (browser *Browser) AccessibilityTree(ctx context.Context) ([]map[string]any, error) {
	var response struct {
		Nodes []map[string]any `json:"nodes"`
	}
	err := browser.Call(ctx, "Accessibility.getFullAXTree", map[string]any{}, &response)
	return response.Nodes, err
}

// Screenshot writes a PNG of the current page.
func (browser *Browser) Screenshot(ctx context.Context, path string) error {
	var response struct {
		Data string `json:"data"`
	}
	if err := browser.Call(ctx, "Page.captureScreenshot", map[string]any{"format": "png", "captureBeyondViewport": true}, &response); err != nil {
		return err
	}
	data, err := base64.StdEncoding.DecodeString(response.Data)
	if err != nil {
		return err
	}
	return os.WriteFile(path, data, 0o600)
}

// AssertHealthy fails if the page produced an uncaught exception or console error.
func (browser *Browser) AssertHealthy(t testing.TB) {
	t.Helper()
	browser.eventMu.Lock()
	defer browser.eventMu.Unlock()
	if len(browser.errors) != 0 {
		t.Fatalf("browser errors:\n%s\nconsole:\n%s", strings.Join(browser.errors, "\n"), strings.Join(browser.logs, "\n"))
	}
}

// Close terminates Chromium and closes the DevTools connection.
func (browser *Browser) Close() {
	browser.closeOnce.Do(func() {
		_ = browser.socket.close()
		if browser.command.Process != nil {
			_ = browser.command.Process.Kill()
		}
		select {
		case <-browser.done:
		case <-time.After(3 * time.Second):
		}
	})
}

type webSocket struct {
	connection net.Conn
	reader     *bufio.Reader
	writeMu    sync.Mutex
}

func dialWebSocket(rawURL string) (*webSocket, error) {
	parsed, err := url.Parse(rawURL)
	if err != nil {
		return nil, err
	}
	if parsed.Scheme != "ws" {
		return nil, fmt.Errorf("unsupported WebSocket scheme %q", parsed.Scheme)
	}
	connection, err := net.DialTimeout("tcp", parsed.Host, 5*time.Second)
	if err != nil {
		return nil, err
	}
	keyBytes := make([]byte, 16)
	if _, err := rand.Read(keyBytes); err != nil {
		connection.Close()
		return nil, err
	}
	key := base64.StdEncoding.EncodeToString(keyBytes)
	request := &http.Request{
		Method: http.MethodGet,
		URL:    &url.URL{Scheme: "http", Host: parsed.Host, Path: parsed.Path, RawQuery: parsed.RawQuery},
		Host:   parsed.Host,
		Header: http.Header{
			"Connection":            {"Upgrade"},
			"Upgrade":               {"websocket"},
			"Sec-Websocket-Version": {"13"},
			"Sec-Websocket-Key":     {key},
			"Origin":                {"http://localhost"},
		},
	}
	if err := request.Write(connection); err != nil {
		connection.Close()
		return nil, err
	}
	reader := bufio.NewReader(connection)
	response, err := http.ReadResponse(reader, request)
	if err != nil {
		connection.Close()
		return nil, err
	}
	if response.StatusCode != http.StatusSwitchingProtocols {
		_ = response.Body.Close()
		connection.Close()
		return nil, fmt.Errorf("WebSocket handshake status %d", response.StatusCode)
	}
	sum := sha1.Sum([]byte(key + webSocketGUID))
	expected := base64.StdEncoding.EncodeToString(sum[:])
	if response.Header.Get("Sec-Websocket-Accept") != expected {
		connection.Close()
		return nil, errors.New("invalid WebSocket accept value")
	}
	return &webSocket{connection: connection, reader: reader}, nil
}

func (socket *webSocket) writeText(data []byte) error {
	return socket.writeFrame(0x1, data)
}

func (socket *webSocket) writeFrame(opcode byte, payload []byte) error {
	socket.writeMu.Lock()
	defer socket.writeMu.Unlock()
	var header bytes.Buffer
	header.WriteByte(0x80 | opcode)
	length := len(payload)
	switch {
	case length < 126:
		header.WriteByte(0x80 | byte(length))
	case length <= 65535:
		header.WriteByte(0x80 | 126)
		_ = binary.Write(&header, binary.BigEndian, uint16(length))
	default:
		header.WriteByte(0x80 | 127)
		_ = binary.Write(&header, binary.BigEndian, uint64(length))
	}
	mask := make([]byte, 4)
	if _, err := rand.Read(mask); err != nil {
		return err
	}
	header.Write(mask)
	masked := make([]byte, len(payload))
	for index := range payload {
		masked[index] = payload[index] ^ mask[index%4]
	}
	if _, err := socket.connection.Write(header.Bytes()); err != nil {
		return err
	}
	_, err := socket.connection.Write(masked)
	return err
}

func (socket *webSocket) readText() ([]byte, error) {
	var message []byte
	for {
		first, err := socket.reader.ReadByte()
		if err != nil {
			return nil, err
		}
		second, err := socket.reader.ReadByte()
		if err != nil {
			return nil, err
		}
		fin := first&0x80 != 0
		opcode := first & 0x0f
		masked := second&0x80 != 0
		length := uint64(second & 0x7f)
		switch length {
		case 126:
			var value uint16
			if err := binary.Read(socket.reader, binary.BigEndian, &value); err != nil {
				return nil, err
			}
			length = uint64(value)
		case 127:
			if err := binary.Read(socket.reader, binary.BigEndian, &length); err != nil {
				return nil, err
			}
		}
		if length > 64<<20 {
			return nil, fmt.Errorf("WebSocket frame too large: %s", strconv.FormatUint(length, 10))
		}
		var mask [4]byte
		if masked {
			if _, err := io.ReadFull(socket.reader, mask[:]); err != nil {
				return nil, err
			}
		}
		payload := make([]byte, int(length))
		if _, err := io.ReadFull(socket.reader, payload); err != nil {
			return nil, err
		}
		if masked {
			for index := range payload {
				payload[index] ^= mask[index%4]
			}
		}
		switch opcode {
		case 0x8:
			return nil, io.EOF
		case 0x9:
			if err := socket.writeFrame(0xA, payload); err != nil {
				return nil, err
			}
			continue
		case 0xA:
			continue
		case 0x0, 0x1:
			message = append(message, payload...)
			if fin {
				return message, nil
			}
		default:
			if fin && len(message) != 0 {
				return message, nil
			}
		}
	}
}

func (socket *webSocket) close() error {
	_ = socket.writeFrame(0x8, []byte{})
	return socket.connection.Close()
}
