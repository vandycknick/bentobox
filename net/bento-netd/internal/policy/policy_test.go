package policy

import (
	"errors"
	"net"
	"net/http"
	"strings"
	"testing"
)

func TestDefaultActionDefaultsToAllow(t *testing.T) {
	compiled := loadPolicy(t, `
endpoint "ip" "private" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = [443]
}
`)

	decision := compiled.EvaluateFlow(Flow{
		Protocol: "tcp",
		SourceIP: net.ParseIP("192.168.127.2"),
		DestIP:   net.ParseIP("192.0.2.10"),
		DestPort: 443,
	})
	if decision.Action != ActionAllow || decision.Source != DecisionSourceDefault {
		t.Fatalf("expected default allow, got %#v", decision)
	}
}

func TestFlowRulesUsePriorityThenDeclarationOrder(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  default_action = "deny"
}

endpoint "ip" "private" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = [443]
}

rule "lower-deny" {
  endpoint = ip.private
  verdict = "deny"
  priority = 10
}

rule "first-allow" {
  endpoint = ip.private
  verdict = "allow"
  priority = 20
}

rule "second-deny" {
  endpoint = ip.private
  verdict = "deny"
  priority = 20
}
`)

	decision := compiled.EvaluateFlow(Flow{
		Protocol: "tcp",
		SourceIP: net.ParseIP("192.168.127.2"),
		DestIP:   net.ParseIP("10.1.2.3"),
		DestPort: 443,
	})
	if decision.Action != ActionAllow || decision.RuleName != "first-allow" {
		t.Fatalf("expected first high-priority allow, got %#v", decision)
	}
}

func TestDisabledRulesAreValidatedButNotEvaluated(t *testing.T) {
	compiled := loadPolicy(t, `
endpoint "ip" "private" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = [443]
}

rule "disabled-deny" {
  endpoint = ip.private
  verdict = "deny"
  disabled = true
  priority = 100
}

rule "allow" {
  endpoint = ip.private
  verdict = "allow"
  priority = 1
}
`)

	decision := compiled.EvaluateFlow(Flow{
		Protocol: "tcp",
		SourceIP: net.ParseIP("192.168.127.2"),
		DestIP:   net.ParseIP("10.1.2.3"),
		DestPort: 443,
	})
	if decision.Action != ActionAllow || decision.RuleName != "allow" {
		t.Fatalf("expected disabled rule to be skipped, got %#v", decision)
	}

	_, err := loadPolicyError(t, `
endpoint "ip" "private" {
  destination = ["10.0.0.0/8"]
}

rule "disabled-invalid" {
  endpoint = ip.private
  verdict = "deny"
  condition = "http.method == 'GET'"
  disabled = true
}
`)
	if err == nil || !strings.Contains(err.Error(), "condition") {
		t.Fatalf("expected disabled invalid rule to fail validation, got %v", err)
	}
}

func TestHTTPFamilyRulesMayMixHTTPAndHTTPS(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  default_action = "deny"
}

endpoint "http" "metadata" {
  hosts = ["metadata.internal"]
}

endpoint "https" "github" {
  hosts = ["api.github.com"]
}

rule "http-family-read" {
  endpoints = [http.metadata, https.github]
  condition = "http.method in ['GET', 'HEAD']"
  verdict = "allow"
}
`)

	cleartext := compiled.EvaluateHTTP(HTTPRequest{EndpointKind: "http", Host: "metadata.internal", Method: http.MethodGet, Path: "/latest"})
	if cleartext.Action != ActionAllow || cleartext.EndpointKind != "http" {
		t.Fatalf("expected http allow, got %#v", cleartext)
	}
	https := compiled.EvaluateHTTP(HTTPRequest{EndpointKind: "https", Host: "api.github.com:443", Method: http.MethodGet, Path: "/repos"})
	if https.Action != ActionAllow || https.EndpointKind != "https" {
		t.Fatalf("expected https allow, got %#v", https)
	}
}

func TestReferencesAllowHCLIdentifiersWithDashes(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  default_action = "deny"
}

endpoint "https" "openai-codex" {
  hosts = ["chatgpt.com"]
}

credential "bearer_token" "api-token" {
  endpoint = https.openai-codex
}

rule "allow-codex" {
  endpoint = https.openai-codex
  credential = bearer_token.api-token
  verdict = "allow"
}
`)

	decision := compiled.EvaluateHTTP(HTTPRequest{EndpointKind: "https", Host: "chatgpt.com", Method: http.MethodGet})
	if decision.Action != ActionAllow || decision.RuleName != "allow-codex" {
		t.Fatalf("expected dashed endpoint reference to allow, got %#v", decision)
	}
	if decision.EndpointName != "openai-codex" {
		t.Fatalf("expected dashed endpoint name, got %q", decision.EndpointName)
	}
	if decision.SelectedCredential == nil || decision.SelectedCredential.Name != "api-token" {
		t.Fatalf("expected dashed credential name, got %#v", decision.SelectedCredential)
	}
}

func TestMixedEndpointFamiliesAreRejected(t *testing.T) {
	_, err := loadPolicyError(t, `
endpoint "ip" "private" {
  destination = ["10.0.0.0/8"]
}

endpoint "https" "github" {
  hosts = ["api.github.com"]
}

rule "mixed" {
  endpoints = [ip.private, https.github]
  verdict = "allow"
}
`)
	if err == nil || !strings.Contains(err.Error(), "same family") {
		t.Fatalf("expected mixed family error, got %v", err)
	}
}

func TestUnknownFieldsAndUnsupportedSyntaxAreRejected(t *testing.T) {
	_, err := loadPolicyError(t, `
endpoint "invalid_endpoint" "private" {
  destination = ["10.0.0.0/8"]
}
`)
	if err == nil || !strings.Contains(err.Error(), `unsupported endpoint kind "invalid_endpoint"`) {
		t.Fatalf("expected unsupported endpoint kind error, got %v", err)
	}

	_, err = loadPolicyError(t, `
settings {
  surprise = "/tmp/nope"
}
`)
	if err == nil || !strings.Contains(err.Error(), "surprise") {
		t.Fatalf("expected unknown settings field error, got %v", err)
	}

	_, err = loadPolicyError(t, `
endpoint "ip" "private" {
  surprise = ["10.0.0.0/8"]
}
`)
	if err == nil || !strings.Contains(err.Error(), "surprise") {
		t.Fatalf("expected unknown endpoint field error, got %v", err)
	}
}

func TestPortNumbersMustBeIntegers(t *testing.T) {
	_, err := loadPolicyError(t, `
endpoint "ip" "private" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = [443.5]
}
`)
	if err == nil || !strings.Contains(err.Error(), "port 443.5 must be an integer") {
		t.Fatalf("expected fractional port error, got %v", err)
	}
}

func TestIPEndpointExactTCPPortMatches(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  default_action = "deny"
}

endpoint "ip" "private" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = [443]
}

rule "allow-private" {
  endpoint = ip.private
  verdict = "allow"
}
`)

	decision := compiled.EvaluateFlow(l4Flow("tcp", 443))
	if decision.Action != ActionAllow || decision.RuleName != "allow-private" {
		t.Fatalf("expected tcp 443 allow, got %#v", decision)
	}
	assertL4Match(t, decision, L4Match{EndpointProtocol: "tcp", DestPort: 443, PortRange: PortRange{Start: 443, End: 443}, Kind: L4MatchExactPort})

	decision = compiled.EvaluateFlow(l4Flow("tcp", 444))
	if decision.Action != ActionDeny || decision.Source != DecisionSourceDefault {
		t.Fatalf("expected tcp 444 default deny, got %#v", decision)
	}
	if decision.MatchedL4 != nil {
		t.Fatalf("default decision must not carry l4 metadata, got %#v", decision.MatchedL4)
	}
}

func TestIPEndpointExactUDPPortMatches(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  default_action = "deny"
}

endpoint "ip" "dns" {
  destination = ["10.0.0.0/8"]
  protocol = "udp"
  ports = [53]
}

rule "allow-dns" {
  endpoint = ip.dns
  verdict = "allow"
}
`)

	decision := compiled.EvaluateFlow(l4Flow("udp", 53))
	if decision.Action != ActionAllow || decision.RuleName != "allow-dns" {
		t.Fatalf("expected udp 53 allow, got %#v", decision)
	}
	assertL4Match(t, decision, L4Match{EndpointProtocol: "udp", DestPort: 53, PortRange: PortRange{Start: 53, End: 53}, Kind: L4MatchExactPort})

	decision = compiled.EvaluateFlow(l4Flow("tcp", 53))
	if decision.Action != ActionDeny || decision.Source != DecisionSourceDefault {
		t.Fatalf("expected tcp 53 to miss udp endpoint, got %#v", decision)
	}
}

func TestIPEndpointPortRangesAreInclusive(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  default_action = "deny"
}

endpoint "ip" "app" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = ["8000-8002"]
}

rule "allow-app" {
  endpoint = ip.app
  verdict = "allow"
}
`)

	for _, port := range []uint16{8000, 8001, 8002} {
		decision := compiled.EvaluateFlow(l4Flow("tcp", port))
		if decision.Action != ActionAllow || decision.RuleName != "allow-app" {
			t.Fatalf("expected tcp %d allow, got %#v", port, decision)
		}
		assertL4Match(t, decision, L4Match{EndpointProtocol: "tcp", DestPort: port, PortRange: PortRange{Start: 8000, End: 8002}, Kind: L4MatchRange})
	}

	for _, port := range []uint16{7999, 8003} {
		decision := compiled.EvaluateFlow(l4Flow("tcp", port))
		if decision.Action != ActionDeny || decision.Source != DecisionSourceDefault {
			t.Fatalf("expected tcp %d default deny, got %#v", port, decision)
		}
	}
}

func TestIPEndpointBoundaryPortsMatch(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  default_action = "deny"
}

endpoint "ip" "boundaries" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = [1, 65535]
}

rule "allow-boundaries" {
  endpoint = ip.boundaries
  verdict = "allow"
}
`)

	for _, port := range []uint16{1, 65535} {
		decision := compiled.EvaluateFlow(l4Flow("tcp", port))
		if decision.Action != ActionAllow || decision.RuleName != "allow-boundaries" {
			t.Fatalf("expected tcp %d allow, got %#v", port, decision)
		}
		assertL4Match(t, decision, L4Match{EndpointProtocol: "tcp", DestPort: port, PortRange: PortRange{Start: port, End: port}, Kind: L4MatchExactPort})
	}
}

func TestIPEndpointDefaultProtocolMatchesAnyWithoutPorts(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  default_action = "deny"
}

endpoint "ip" "private" {
  destination = ["10.0.0.0/8"]
}

rule "allow-private" {
  endpoint = ip.private
  verdict = "allow"
}
`)

	for _, protocol := range []string{"tcp", "udp"} {
		decision := compiled.EvaluateFlow(l4Flow(protocol, 8443))
		if decision.Action != ActionAllow || decision.RuleName != "allow-private" {
			t.Fatalf("expected %s flow allow, got %#v", protocol, decision)
		}
		assertL4Match(t, decision, L4Match{EndpointProtocol: "any", DestPort: 8443, Kind: L4MatchProtocolOnly})
	}
}

func TestIPEndpointPortRangesAreCanonicalized(t *testing.T) {
	compiled := loadPolicy(t, `
endpoint "ip" "canonical" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = [443, "8000-8002", "8001-8003", 443, "444-445", "446-446"]
}
`)

	endpoint := compiled.ipEndpoints["ip.canonical"]
	if endpoint == nil {
		t.Fatal("expected ip.canonical endpoint")
	}
	want := []PortRange{{Start: 443, End: 446}, {Start: 8000, End: 8003}}
	if len(endpoint.Ports) != len(want) {
		t.Fatalf("expected canonical ports %#v, got %#v", want, endpoint.Ports)
	}
	for i := range want {
		if endpoint.Ports[i] != want[i] {
			t.Fatalf("expected canonical ports %#v, got %#v", want, endpoint.Ports)
		}
	}
}

func TestInvalidIPEndpointL4PolicyIsRejected(t *testing.T) {
	tests := []struct {
		name string
		body string
		want string
	}{
		{
			name: "ports default to protocol any",
			body: `
endpoint "ip" "bad" {
  destination = ["10.0.0.0/8"]
  ports = [443]
}
`,
			want: "protocol any cannot be combined with ports",
		},
		{
			name: "ports with protocol any",
			body: `
endpoint "ip" "bad" {
  destination = ["10.0.0.0/8"]
  protocol = "any"
  ports = [443]
}
`,
			want: "protocol any cannot be combined with ports",
		},
		{
			name: "port zero",
			body: `
endpoint "ip" "bad" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = [0]
}
`,
			want: "port 0 is out of range",
		},
		{
			name: "port too high",
			body: `
endpoint "ip" "bad" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = [65536]
}
`,
			want: "port 65536 is out of range",
		},
		{
			name: "reversed range",
			body: `
endpoint "ip" "bad" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = ["9000-8000"]
}
`,
			want: `port range "9000-8000" ends before it starts`,
		},
		{
			name: "malformed range",
			body: `
endpoint "ip" "bad" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = ["8000-9000-10000"]
}
`,
			want: `invalid port range "8000-9000-10000"`,
		},
		{
			name: "quoted exact port",
			body: `
endpoint "ip" "bad" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = ["443"]
}
`,
			want: `invalid port range "443"`,
		},
		{
			name: "non integer range",
			body: `
endpoint "ip" "bad" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = ["53.5-54"]
}
`,
			want: `port "53.5" is out of range`,
		},
		{
			name: "unsupported protocol",
			body: `
endpoint "ip" "bad" {
  destination = ["10.0.0.0/8"]
  protocol = "icmp"
}
`,
			want: `unsupported protocol "icmp"`,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			_, err := loadPolicyError(t, test.body)
			if err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("expected error containing %q, got %v", test.want, err)
			}
		})
	}
}

func TestExplicitIPDenyIsTerminalBeforeL7Classification(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  default_action = "deny"
}

endpoint "ip" "blocked" {
  destination = ["203.0.113.0/24"]
  protocol = "tcp"
  ports = [443]
}

endpoint "https" "api" {
  hosts = ["api.example.com"]
}

rule "block" {
  endpoint = ip.blocked
  verdict = "deny"
  priority = 100
}

rule "allow-api" {
  endpoint = https.api
  verdict = "allow"
}
`)

	decision := compiled.EvaluateFlow(Flow{
		Protocol: "tcp",
		SourceIP: net.ParseIP("192.168.127.2"),
		DestIP:   net.ParseIP("203.0.113.10"),
		DestPort: 443,
	})
	if decision.Action != ActionDeny || decision.Source != DecisionSourceRule || decision.ClassificationOpportunity {
		t.Fatalf("expected terminal ip deny, got %#v", decision)
	}
}

func TestDefaultDenyAllowsL7ClassificationOnly(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  default_action = "deny"
}

endpoint "http" "metadata" {
  hosts = ["metadata.internal"]
}

rule "allow-metadata" {
  endpoint = http.metadata
  verdict = "allow"
}
`)

	flowDecision := compiled.EvaluateFlow(Flow{
		Protocol: "tcp",
		SourceIP: net.ParseIP("192.168.127.2"),
		DestIP:   net.ParseIP("169.254.169.254"),
		DestPort: 80,
	})
	if flowDecision.Action != ActionDeny || flowDecision.Source != DecisionSourceDefault || !flowDecision.ClassificationOpportunity {
		t.Fatalf("expected default-deny classification opportunity, got %#v", flowDecision)
	}

	requestDecision := compiled.EvaluateHTTP(HTTPRequest{EndpointKind: "http", Host: "metadata.internal", Method: http.MethodGet, Path: "/latest"})
	if requestDecision.Action != ActionAllow || requestDecision.Source != DecisionSourceRule {
		t.Fatalf("expected L7 rule allow after classification, got %#v", requestDecision)
	}
}

func TestHTTPSClassificationUsesConfiguredEndpointPorts(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  default_action = "deny"
}

endpoint "https" "api" {
  hosts = ["api.example.com:8443"]
}
`)

	if !compiled.ShouldInterceptHTTPS(8443) {
		t.Fatal("expected explicitly configured https port 8443 to be intercepted")
	}
	if compiled.ShouldInterceptHTTPS(443) {
		t.Fatal("did not expect port 443 interception without a port 443 https binding")
	}

	decision := compiled.EvaluateFlow(Flow{Protocol: "tcp", SourceIP: net.ParseIP("192.168.127.2"), DestIP: net.ParseIP("203.0.113.10"), DestPort: 8443})
	if decision.Action != ActionDeny || decision.Source != DecisionSourceDefault || !decision.ClassificationOpportunity {
		t.Fatalf("expected default-deny classification on configured port, got %#v", decision)
	}

	decision = compiled.EvaluateFlow(Flow{Protocol: "tcp", SourceIP: net.ParseIP("192.168.127.2"), DestIP: net.ParseIP("203.0.113.10"), DestPort: 443})
	if decision.Action != ActionDeny || decision.ClassificationOpportunity {
		t.Fatalf("expected unconfigured port to remain flow default deny, got %#v", decision)
	}
}

func TestResolveHTTPSRawIPMatchesOnlyExactIPBindings(t *testing.T) {
	compiled := loadPolicy(t, `
endpoint "https" "proxmox" {
  hosts = ["203.0.113.10:8006"]
}

endpoint "https" "api" {
  hosts = ["api.example.com", "*.example.net"]
}
`)

	ref, authority, certHost, ok := compiled.ResolveHTTPSRawIP(net.ParseIP("203.0.113.10"), 8006)
	if !ok || ref.Name != "proxmox" || authority != "203.0.113.10:8006" || certHost != "203.0.113.10" {
		t.Fatalf("raw IP resolution = (%#v, %q, %q, %v), want proxmox 203.0.113.10:8006 203.0.113.10 true", ref, authority, certHost, ok)
	}
	if _, _, _, ok := compiled.ResolveHTTPSRawIP(net.ParseIP("203.0.113.10"), 8443); ok {
		t.Fatal("did not expect raw IP binding to match the wrong port")
	}
	if _, _, _, ok := compiled.ResolveHTTPSRawIP(net.ParseIP("203.0.113.11"), 8006); ok {
		t.Fatal("did not expect raw IP binding to match the wrong IP")
	}
}

func TestConditionRuntimeErrorsFailClosed(t *testing.T) {
	compiled := loadPolicy(t, `
endpoint "https" "api" {
  hosts = ["api.example.com"]
}

rule "bad-condition" {
  endpoint = https.api
  condition = "http.headers['x-missing'][0] == 'yes'"
  verdict = "allow"
}
`)

	decision := compiled.EvaluateHTTP(HTTPRequest{EndpointKind: "https", Host: "api.example.com", Method: http.MethodGet, Path: "/"})
	if decision.Action != ActionDeny || decision.Reason != "condition_error" {
		t.Fatalf("expected condition error deny, got %#v", decision)
	}
}

func TestCredentialMetadataDoesNotApplyOnDefaultAllow(t *testing.T) {
	compiled := loadPolicy(t, `
endpoint "https" "api" {
  hosts = ["api.example.com"]
}

credential "bearer_token" "api" {
  endpoint = https.api
}
`)

	decision := compiled.EvaluateHTTP(HTTPRequest{EndpointKind: "https", Host: "api.example.com", Method: http.MethodGet, Path: "/"})
	if decision.Action != ActionAllow || decision.Source != DecisionSourceDefault {
		t.Fatalf("expected default allow, got %#v", decision)
	}
	if decision.SelectedCredential != nil {
		t.Fatalf("default allow must not select credentials, got %#v", decision.SelectedCredential)
	}
}

func TestAuditSettingsWarningsArePolicyLoadWarnings(t *testing.T) {
	compiled := loadPolicy(t, `
settings {
  audit {
    body_buffer = "1KiB"
    body_storage = "4KiB"
  }
}
`)
	if len(compiled.Warnings()) != 1 || !strings.Contains(compiled.Warnings()[0], "body_buffer") {
		t.Fatalf("expected audit warning, got %#v", compiled.Warnings())
	}
}

func TestLoadFileReportsMultipleDiagnosticsWithRanges(t *testing.T) {
	_, err := loadPolicyError(t, `
settings {
  surprise = "/tmp/nope"
}

endpoint "invalid_endpoint" "private" {
  destination = ["10.0.0.0/8"]
}

endpoint "https" "api" {
  secret = "not-here"
}

credential "bearer_token" "api" {
  endpoint = https.api
  secret = "api-token"
}
`)
	if err == nil {
		t.Fatal("expected invalid policy to fail")
	}
	var loadErr *LoadError
	if !errors.As(err, &loadErr) {
		t.Fatalf("expected LoadError, got %T", err)
	}
	if len(loadErr.Diagnostics) < 4 {
		t.Fatalf("expected multiple diagnostics, got %#v", loadErr.Diagnostics)
	}
	expected := `load policy file policy.hcl failed with 5 errors:
policy.hcl:3:3: Unsupported argument
  An argument named "surprise" is not expected here.
policy.hcl:6:10: Unsupported endpoint kind
  unsupported endpoint kind "invalid_endpoint"
policy.hcl:11:3: Unsupported argument
  An argument named "secret" is not expected here.
policy.hcl:10:1: Missing hosts
  hosts is required
policy.hcl:16:3: Unsupported argument
  An argument named "secret" is not expected here.`
	if err.Error() != expected {
		t.Fatalf("unexpected error text\nwant:\n%s\n got:\n%s", expected, err.Error())
	}
}

func loadPolicy(t *testing.T, text string) *Policy {
	t.Helper()
	compiled, err := loadPolicyError(t, text)
	if err != nil {
		t.Fatalf("LoadFile returned error: %v", err)
	}
	return compiled
}

func loadPolicyError(t *testing.T, text string) (*Policy, error) {
	t.Helper()
	return LoadReader("policy.hcl", strings.NewReader(text))
}

func l4Flow(protocol string, destPort uint16) Flow {
	return Flow{
		Protocol: protocol,
		SourceIP: net.ParseIP("192.168.127.2"),
		DestIP:   net.ParseIP("10.1.2.3"),
		DestPort: destPort,
	}
}

func assertL4Match(t *testing.T, decision Decision, want L4Match) {
	t.Helper()
	if decision.MatchedL4 == nil {
		t.Fatalf("expected l4 match %#v, got nil", want)
	}
	if *decision.MatchedL4 != want {
		t.Fatalf("expected l4 match %#v, got %#v", want, *decision.MatchedL4)
	}
}
