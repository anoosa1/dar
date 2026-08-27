package outbound

import (
	"context"
	"net"
	"testing"
)

func FuzzOutboundURLValidation(f *testing.F) {
	for _, seed := range []string{
		"https://example.test/feed",
		"http://127.0.0.1/private",
		"https://user:password@example.test/",
		"file:///etc/passwd",
		"://",
	} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, raw string) {
		if len(raw) > 64<<10 {
			return
		}
		policy := Policy{
			AllowedHosts: map[string]struct{}{},
			Resolver:     fixedFuzzResolver{ip: net.ParseIP("203.0.113.10")},
		}
		_, _ = policy.Validate(context.Background(), raw)
	})
}

type fixedFuzzResolver struct{ ip net.IP }

func (r fixedFuzzResolver) LookupIP(context.Context, string, string) ([]net.IP, error) {
	return []net.IP{r.ip}, nil
}
