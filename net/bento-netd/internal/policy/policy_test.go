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
