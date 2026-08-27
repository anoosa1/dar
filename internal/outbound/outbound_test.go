package outbound

import (
	"context"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
)

type sequenceResolver struct {
	mu      sync.Mutex
	results [][]net.IP
	err     error
}

func (r *sequenceResolver) LookupIP(context.Context, string, string) ([]net.IP, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.err != nil {
		return nil, r.err
	}
	if len(r.results) == 0 {
		return []net.IP{net.ParseIP("127.0.0.1")}, nil
	}
	out := r.results[0]
	if len(r.results) > 1 {
		r.results = r.results[1:]
	}
	return out, nil
}

func TestPolicyParsingPublicTargetsAndAllowedCIDRClient(t *testing.T) {
	if _, err := ParsePolicy("", "not-a-cidr"); err == nil {
		t.Fatal("invalid CIDR accepted")
	}
	public, err := ParsePolicy("", "")
	if err != nil {
		t.Fatal(err)
	}
	public.Resolver = &sequenceResolver{results: [][]net.IP{{net.ParseIP("8.8.8.8")}}}
	if _, err := public.Validate(context.Background(), "https://public.test/feed"); err != nil {
		t.Fatalf("public target rejected: %v", err)
	}
	public.Resolver = &sequenceResolver{err: errors.New("DNS unavailable")}
	if _, err := public.Validate(context.Background(), "https://public.test/feed"); err == nil {
		t.Fatal("resolver failure ignored")
	}

	fixture := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("ok")) }))
	defer fixture.Close()
	allowed, err := ParsePolicy("", "127.0.0.0/8")
	if err != nil {
		t.Fatal(err)
	}
	response, err := allowed.Client(0).Get(fixture.URL)
	if err != nil {
		t.Fatalf("allowed client request: %v", err)
	}
	_ = response.Body.Close()
	if response.StatusCode != http.StatusOK {
		t.Fatalf("status=%d", response.StatusCode)
	}
	request := httptest.NewRequest(http.MethodGet, "https://public.test", nil)
	via := make([]*http.Request, 8)
	if err := allowed.Client(0).CheckRedirect(request, via); err == nil {
		t.Fatal("redirect limit ignored")
	}
	if _, err := allowed.ValidateHost(context.Background(), "bad/host"); err == nil {
		t.Fatal("invalid bare host accepted")
	}
}

func TestValidateBlocksPrivateTargetsAndCredentials(t *testing.T) {
	p, err := ParsePolicy("", "")
	if err != nil {
		t.Fatal(err)
	}
	p.Resolver = &sequenceResolver{results: [][]net.IP{{net.ParseIP("127.0.0.1")}}}
	for _, raw := range []string{"http://internal.test", "ftp://public.test/file", "https://user:pass@public.test"} {
		if _, err := p.Validate(context.Background(), raw); err == nil {
			t.Fatalf("Validate(%q) succeeded", raw)
		}
	}
	allowed, err := ParsePolicy("internal.test", "")
	if err != nil {
		t.Fatal(err)
	}
	allowed.Resolver = p.Resolver
	if _, err := allowed.Validate(context.Background(), "https://internal.test/feed"); err != nil {
		t.Fatalf("explicit host allowlist rejected: %v", err)
	}
}

func TestClientRevalidatesAtDialAndRedirect(t *testing.T) {
	p, err := ParsePolicy("", "")
	if err != nil {
		t.Fatal(err)
	}
	p.Resolver = &sequenceResolver{results: [][]net.IP{
		{net.ParseIP("203.0.113.10")},
		{net.ParseIP("127.0.0.1")},
	}}
	if _, err := p.Validate(context.Background(), "https://rebind.test"); err != nil {
		t.Fatal(err)
	}
	req := httptest.NewRequest(http.MethodGet, "https://rebind.test", nil)
	_, err = p.Client(0).Transport.RoundTrip(req)
	if err == nil || !strings.Contains(err.Error(), "blocked") {
		t.Fatalf("rebinding dial error = %v", err)
	}

	redirect := httptest.NewRequest(http.MethodGet, "http://private.test", nil)
	p.Resolver = &sequenceResolver{results: [][]net.IP{{net.ParseIP("10.0.0.1")}}}
	if err := p.Client(0).CheckRedirect(redirect, nil); err == nil {
		t.Fatal("redirect to private address was accepted")
	}
}
