package email

import (
	"context"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/outbound"
)

type autoServer struct {
	Type       string `xml:"type,attr"`
	Hostname   string `xml:"hostname"`
	Port       int    `xml:"port"`
	SocketType string `xml:"socketType"`
	Username   string `xml:"username"`
}
type autoConfig struct {
	Provider struct {
		Incoming []autoServer `xml:"incomingServer"`
		Outgoing []autoServer `xml:"outgoingServer"`
	} `xml:"emailProvider"`
}
type DiscoveryResult struct {
	Email        string       `json:"email"`
	Username     string       `json:"username"`
	IMAPHost     string       `json:"imap_host"`
	IMAPPort     int          `json:"imap_port"`
	IMAPSecurity securityMode `json:"imap_security"`
	SMTPHost     string       `json:"smtp_host"`
	SMTPPort     int          `json:"smtp_port"`
	SMTPSecurity securityMode `json:"smtp_security"`
	Source       string       `json:"source"`
	Candidates   []Account    `json:"candidates,omitempty"`
}

func socketMode(v string, port int) securityMode {
	switch strings.ToUpper(v) {
	case "SSL", "SSL/TLS", "IMAPS":
		return securityTLS
	case "STARTTLS":
		return securitySTARTTLS
	}
	if port == 993 || port == 465 {
		return securityTLS
	}
	if port == 143 || port == 587 {
		return securitySTARTTLS
	}
	return securityNone
}
func expandUsername(v, email string) string {
	local := strings.SplitN(email, "@", 2)[0]
	v = strings.ReplaceAll(v, "%EMAILADDRESS%", email)
	v = strings.ReplaceAll(v, "%EMAILLOCALPART%", local)
	if v == "" {
		v = email
	}
	return v
}
func parseAutoConfig(data []byte, email, source string) (DiscoveryResult, error) {
	var cfg autoConfig
	if err := xml.Unmarshal(data, &cfg); err != nil {
		return DiscoveryResult{}, err
	}
	var imap, smtp *autoServer
	for i := range cfg.Provider.Incoming {
		if strings.EqualFold(cfg.Provider.Incoming[i].Type, "imap") {
			imap = &cfg.Provider.Incoming[i]
			break
		}
	}
	for i := range cfg.Provider.Outgoing {
		if strings.EqualFold(cfg.Provider.Outgoing[i].Type, "smtp") {
			smtp = &cfg.Provider.Outgoing[i]
			break
		}
	}
	if imap == nil || smtp == nil || imap.Hostname == "" || smtp.Hostname == "" {
		return DiscoveryResult{}, errors.New("autoconfig has no IMAP/SMTP pair")
	}
	return DiscoveryResult{Email: email, Username: expandUsername(imap.Username, email), IMAPHost: imap.Hostname, IMAPPort: imap.Port, IMAPSecurity: socketMode(imap.SocketType, imap.Port), SMTPHost: smtp.Hostname, SMTPPort: smtp.Port, SMTPSecurity: socketMode(smtp.SocketType, smtp.Port), Source: source}, nil
}

func discoveryClient(policies ...outbound.Policy) *http.Client {
	var policy outbound.Policy
	if len(policies) > 0 {
		policy = policies[0]
	}
	client := policy.Client(6 * time.Second)
	baseRedirect := client.CheckRedirect
	client.CheckRedirect = func(req *http.Request, via []*http.Request) error {
		if len(via) >= 3 {
			return errors.New("too many redirects")
		}
		if req.URL.Scheme != "https" {
			return errors.New("autoconfig redirects must use HTTPS")
		}
		return baseRedirect(req, via)
	}
	return client
}
func fetchAuto(ctx context.Context, rawURL, email, source string, policy outbound.Policy) (DiscoveryResult, error) {
	if _, err := policy.Validate(ctx, rawURL); err != nil {
		return DiscoveryResult{}, err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, rawURL, nil)
	if err != nil {
		return DiscoveryResult{}, err
	}
	req.Header.Set("User-Agent", "Dar Email Autoconfig/1.0")
	resp, err := discoveryClient(policy).Do(req)
	if err != nil {
		return DiscoveryResult{}, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return DiscoveryResult{}, fmt.Errorf("autoconfig status %d", resp.StatusCode)
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, (1<<20)+1))
	if err != nil {
		return DiscoveryResult{}, err
	}
	if len(data) > 1<<20 {
		return DiscoveryResult{}, errors.New("autoconfig response exceeds 1 MiB")
	}
	return parseAutoConfig(data, email, source)
}

type discoveryDeps struct {
	fetch     func(context.Context, string, string, string, outbound.Policy) (DiscoveryResult, error)
	lookupSRV func(context.Context, string, string, string) (string, []*net.SRV, error)
}

func discoverAccount(ctx context.Context, email string, policies ...outbound.Policy) (DiscoveryResult, error) {
	deps := discoveryDeps{
		fetch: fetchAuto,
		lookupSRV: func(ctx context.Context, service, proto, name string) (string, []*net.SRV, error) {
			return net.DefaultResolver.LookupSRV(ctx, service, proto, name)
		},
	}
	return discoverAccountWithDeps(ctx, email, deps, policies...)
}

func discoverAccountWithDeps(ctx context.Context, email string, deps discoveryDeps, policies ...outbound.Policy) (DiscoveryResult, error) {
	var policy outbound.Policy
	if len(policies) > 0 {
		policy = policies[0]
	}
	ctx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()
	email = strings.TrimSpace(strings.ToLower(email))
	parts := strings.Split(email, "@")
	if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
		return DiscoveryResult{}, errors.New("valid email is required")
	}
	domain := parts[1]
	endpoints := []struct{ url, source string }{{"https://autoconfig." + domain + "/mail/config-v1.1.xml", "provider-autoconfig"}, {"https://" + domain + "/.well-known/autoconfig/mail/config-v1.1.xml", "provider-well-known"}}
	for _, x := range endpoints {
		if result, err := deps.fetch(ctx, x.url, email, x.source, policy); err == nil {
			return result, nil
		}
	}
	_, imaps, _ := deps.lookupSRV(ctx, "imaps", "tcp", domain)
	_, imap, _ := deps.lookupSRV(ctx, "imap", "tcp", domain)
	_, submissions, _ := deps.lookupSRV(ctx, "submissions", "tcp", domain)
	_, submission, _ := deps.lookupSRV(ctx, "submission", "tcp", domain)
	pick := func(items []*net.SRV) (string, int) {
		if len(items) == 0 {
			return "", 0
		}
		return strings.TrimSuffix(items[0].Target, "."), int(items[0].Port)
	}
	ih, ip := pick(imaps)
	is := securityTLS
	if ih == "" {
		ih, ip = pick(imap)
		is = securitySTARTTLS
	}
	sh, sp := pick(submissions)
	ss := securityTLS
	if sh == "" {
		sh, sp = pick(submission)
		ss = securitySTARTTLS
	}
	if ih != "" && sh != "" {
		return DiscoveryResult{Email: email, Username: email, IMAPHost: ih, IMAPPort: ip, IMAPSecurity: is, SMTPHost: sh, SMTPPort: sp, SMTPSecurity: ss, Source: "dns-srv"}, nil
	}
	if result, err := deps.fetch(ctx, "https://autoconfig.thunderbird.net/v1.1/"+url.PathEscape(domain), email, "thunderbird-ispdb", policy); err == nil {
		return result, nil
	}
	return DiscoveryResult{Email: email, Username: email, IMAPHost: "imap." + domain, IMAPPort: 993, IMAPSecurity: securityTLS, SMTPHost: "smtp." + domain, SMTPPort: 587, SMTPSecurity: securitySTARTTLS, Source: "heuristic", Candidates: []Account{{Email: email, Username: email, IMAPHost: "imap." + domain, IMAPPort: 993, IMAPSecurity: securityTLS, SMTPHost: "smtp." + domain, SMTPPort: 465, SMTPSecurity: securityTLS}, {Email: email, Username: email, IMAPHost: "imap." + domain, IMAPPort: 993, IMAPSecurity: securityTLS, SMTPHost: "smtp." + domain, SMTPPort: 587, SMTPSecurity: securitySTARTTLS}}}, nil
}

func parsePort(v string, def int) int {
	n, _ := strconv.Atoi(v)
	if n <= 0 {
		return def
	}
	return n
}
