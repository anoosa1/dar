// Package outbound provides SSRF-resistant HTTP clients for operator-configured
// network access. Hostnames are resolved again at dial time and redirects are
// revalidated, preventing validation/dial DNS rebinding gaps.
package outbound

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"strings"
	"time"
)

type resolver interface {
	LookupIP(context.Context, string, string) ([]net.IP, error)
}

// Policy controls exceptional access to otherwise blocked destinations.
type Policy struct {
	AllowedHosts map[string]struct{}
	AllowedCIDRs []*net.IPNet
	Resolver     resolver
}

// ParsePolicy parses comma-separated exact hostnames and CIDR ranges.
func ParsePolicy(hosts, cidrs string) (Policy, error) {
	p := Policy{AllowedHosts: map[string]struct{}{}, Resolver: net.DefaultResolver}
	for _, host := range strings.Split(hosts, ",") {
		host = strings.ToLower(strings.TrimSpace(host))
		if host != "" {
			p.AllowedHosts[host] = struct{}{}
		}
	}
	for _, raw := range strings.Split(cidrs, ",") {
		raw = strings.TrimSpace(raw)
		if raw == "" {
			continue
		}
		_, block, err := net.ParseCIDR(raw)
		if err != nil {
			return Policy{}, fmt.Errorf("invalid outbound CIDR %q: %w", raw, err)
		}
		p.AllowedCIDRs = append(p.AllowedCIDRs, block)
	}
	return p, nil
}

func (p Policy) lookup(ctx context.Context, host string) ([]net.IP, error) {
	resolver := p.Resolver
	if resolver == nil {
		resolver = net.DefaultResolver
	}
	ips, err := resolver.LookupIP(ctx, "ip", host)
	if err != nil || len(ips) == 0 {
		return nil, fmt.Errorf("resolve outbound host %q: %w", host, err)
	}
	return ips, nil
}

func blocked(ip net.IP) bool {
	return ip == nil || ip.IsLoopback() || ip.IsPrivate() || ip.IsLinkLocalUnicast() || ip.IsLinkLocalMulticast() || ip.IsUnspecified() || ip.IsMulticast()
}

func (p Policy) allowed(host string, ip net.IP) bool {
	if _, ok := p.AllowedHosts[strings.ToLower(host)]; ok {
		return true
	}
	for _, block := range p.AllowedCIDRs {
		if block.Contains(ip) {
			return true
		}
	}
	return !blocked(ip)
}

// Validate resolves and validates an HTTP(S) URL against the policy.
func (p Policy) Validate(ctx context.Context, raw string) (*url.URL, error) {
	u, err := url.Parse(strings.TrimSpace(raw))
	if err != nil || u.Hostname() == "" || (u.Scheme != "http" && u.Scheme != "https") || u.User != nil {
		return nil, fmt.Errorf("outbound URL must be HTTP(S) without embedded credentials")
	}
	ips, err := p.lookup(ctx, u.Hostname())
	if err != nil {
		return nil, err
	}
	for _, ip := range ips {
		if !p.allowed(u.Hostname(), ip) {
			return nil, fmt.Errorf("outbound target %s is blocked", ip)
		}
	}
	return u, nil
}

// ValidateHost resolves a bare network host and rejects every address unless
// the destination is public or explicitly allowed by the operator.
func (p Policy) ValidateHost(ctx context.Context, host string) ([]net.IP, error) {
	host = strings.TrimSpace(strings.TrimSuffix(host, "."))
	if host == "" || strings.ContainsAny(host, "/\\@") {
		return nil, fmt.Errorf("invalid outbound host")
	}
	ips, err := p.lookup(ctx, host)
	if err != nil {
		return nil, err
	}
	for _, ip := range ips {
		if !p.allowed(host, ip) {
			return nil, fmt.Errorf("outbound target %s is blocked", ip)
		}
	}
	return ips, nil
}

// DialContext resolves, validates, and connects to the validated IP directly,
// closing the DNS-rebinding gap between a policy check and a socket dial.
func (p Policy) DialContext(ctx context.Context, network, address string) (net.Conn, error) {
	host, port, err := net.SplitHostPort(address)
	if err != nil {
		return nil, err
	}
	ips, err := p.ValidateHost(ctx, host)
	if err != nil {
		return nil, err
	}
	dialer := &net.Dialer{Timeout: 10 * time.Second, KeepAlive: 30 * time.Second}
	var lastErr error
	for _, ip := range ips {
		conn, dialErr := dialer.DialContext(ctx, network, net.JoinHostPort(ip.String(), port))
		if dialErr == nil {
			return conn, nil
		}
		lastErr = dialErr
	}
	return nil, lastErr
}

// Client returns an HTTP client that re-resolves and revalidates every dial and redirect.
func (p Policy) Client(timeout time.Duration) *http.Client {
	transport := &http.Transport{DialContext: p.DialContext}
	return &http.Client{Timeout: timeout, Transport: transport, CheckRedirect: func(req *http.Request, via []*http.Request) error {
		if len(via) >= 8 {
			return fmt.Errorf("too many redirects")
		}
		_, err := p.Validate(req.Context(), req.URL.String())
		return err
	}}
}
