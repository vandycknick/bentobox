package policy

import (
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestLoadFileCompilesCIDRHTTPSAuditAndCredentials(t *testing.T) {
	dir := t.TempDir()
	policyPath := filepath.Join(dir, "policy.hcl")
	writePolicy(t, policyPath, `
settings {
  default_action = "deny"
  audit_log = "/tmp/bento-audit.jsonl"
}

endpoint "cidr" "private" {
  cidrs = ["10.0.0.0/8"]
  protocols = ["tcp"]
  ports = [443]
}

endpoint "https" "github" {
  hosts = ["api.github.com", "*.githubusercontent.com"]
}

credential "bearer_token" "github" {
  endpoint = https.github
  secret = "github-token"
}

rule "audit-private" {
  endpoint = cidr.private
  verdict = "audit"
  priority = 20
  reason = "private range observed"
}

rule "allow-private" {
  endpoint = cidr.private
  verdict = "allow"
  priority = 10
}

rule "audit-github" {
  endpoint = https.github
  verdict = "audit"
  priority = 30
}

rule "github-writes" {
  endpoint = https.github
  condition = "http.method == 'POST'"
  verdict = "deny"
  priority = 20
  reason = "writes blocked"
}

rule "github-reads" {
  endpoint = https.github
  condition = "http.method in ['GET', 'HEAD']"
  verdict = "allow"
  priority = 10
}
`)

	compiled, err := LoadFile(policyPath)
	if err != nil {
		t.Fatalf("LoadFile returned error: %v", err)
	}
	if compiled.AuditLogPath() != "/tmp/bento-audit.jsonl" {
		t.Fatalf("expected audit log path, got %q", compiled.AuditLogPath())
	}

	flowDecision := compiled.EvaluateFlow(Flow{
		Protocol: "tcp",
		SourceIP: net.ParseIP("192.168.127.2"),
		DestIP:   net.ParseIP("10.1.2.3"),
		DestPort: 443,
	})
	if flowDecision.Action != ActionAllow {
		t.Fatalf("expected flow allow, got %s", flowDecision.Action)
	}
	if len(flowDecision.Audits) != 1 || flowDecision.Audits[0].RuleName != "audit-private" {
		t.Fatalf("expected non-terminal audit before allow, got %#v", flowDecision.Audits)
	}

	udpDecision := compiled.EvaluateFlow(Flow{
		Protocol: "udp",
		SourceIP: net.ParseIP("192.168.127.2"),
		DestIP:   net.ParseIP("10.1.2.3"),
		DestPort: 443,
	})
	if udpDecision.Action != ActionDeny {
		t.Fatalf("expected default deny for non-matching udp, got %s", udpDecision.Action)
	}

	readDecision := compiled.EvaluateHTTP(HTTPRequest{
		Host:   "api.github.com:443",
		Method: http.MethodGet,
		Path:   "/repos/nickvan/bentobox",
		Header: http.Header{"X-Test": []string{"1"}},
	})
	if readDecision.Action != ActionAllow {
		t.Fatalf("expected github read allow, got %s", readDecision.Action)
	}
	if len(readDecision.Audits) != 1 || readDecision.Audits[0].RuleName != "audit-github" {
		t.Fatalf("expected https audit before allow, got %#v", readDecision.Audits)
	}
	if readDecision.Credential == nil || readDecision.Credential.Secret != "github-token" {
		t.Fatalf("expected bearer token credential secret, got %#v", readDecision.Credential)
	}

	writeDecision := compiled.EvaluateHTTP(HTTPRequest{
		Host:   "api.github.com",
		Method: http.MethodPost,
		Path:   "/repos/nickvan/bentobox",
		Header: http.Header{},
	})
	if writeDecision.Action != ActionDeny || writeDecision.RuleName != "github-writes" {
		t.Fatalf("expected github write deny, got %#v", writeDecision)
	}
	if writeDecision.Credential != nil {
		t.Fatalf("expected denied github write to skip credential injection, got %#v", writeDecision.Credential)
	}
}

func TestLoadFileRejectsMissingCredentialSecret(t *testing.T) {
	dir := t.TempDir()
	policyPath := filepath.Join(dir, "policy.hcl")
	writePolicy(t, policyPath, `
endpoint "https" "github" {
  hosts = ["api.github.com"]
}

credential "bearer_token" "github" {
  endpoint = https.github
}
`)

	if _, err := LoadFile(policyPath); err == nil {
		t.Fatal("expected missing secret to be rejected")
	}
}

func TestLoadFileCompilesOpenAICodexOAuthCredential(t *testing.T) {
	dir := t.TempDir()
	policyPath := filepath.Join(dir, "policy.hcl")
	writePolicy(t, policyPath, `
endpoint "https" "openai" {
  hosts = ["chatgpt.com"]
}

credential "openai_codex_oauth" "codex" {
  endpoint = https.openai
  secret = "codex-personal"
}

rule "codex" {
  endpoint = https.openai
  verdict = "allow"
}
`)

	compiled, err := LoadFile(policyPath)
	if err != nil {
		t.Fatalf("LoadFile returned error: %v", err)
	}
	decision := compiled.EvaluateHTTP(HTTPRequest{Host: "chatgpt.com", Method: http.MethodPost, Path: "/backend-api/codex/responses"})
	if decision.Credential == nil {
		t.Fatal("expected credential on allow decision")
	}
	if decision.Credential.Kind != "openai_codex_oauth" {
		t.Fatalf("expected openai_codex_oauth credential, got %#v", decision.Credential)
	}
	if decision.Credential.Secret != "codex-personal" {
		t.Fatalf("expected secret %q, got %q", "codex-personal", decision.Credential.Secret)
	}
}

func TestLoadFileRejectsInvalidSecretName(t *testing.T) {
	dir := t.TempDir()
	policyPath := filepath.Join(dir, "policy.hcl")
	writePolicy(t, policyPath, `
endpoint "https" "openai" {
  hosts = ["chatgpt.com"]
}

credential "openai_codex_oauth" "codex" {
  endpoint = https.openai
  secret = "../codex"
}
`)

	_, err := LoadFile(policyPath)
	if err == nil {
		t.Fatal("expected invalid secret name to be rejected")
	}
	if !strings.Contains(err.Error(), "secret") {
		t.Fatalf("expected secret name error, got %v", err)
	}
}

func TestLoadFileRejectsMultipleCredentialsForEndpoint(t *testing.T) {
	dir := t.TempDir()
	policyPath := filepath.Join(dir, "policy.hcl")
	writePolicy(t, policyPath, `
endpoint "https" "github" {
  hosts = ["api.github.com"]
}

credential "bearer_token" "github" {
  endpoint = https.github
  secret = "github-token"
}

credential "bearer_token" "github_backup" {
  endpoint = https.github
  secret = "github-backup-token"
}
`)

	_, err := LoadFile(policyPath)
	if err == nil {
		t.Fatal("expected multiple credentials for one endpoint to be rejected")
	}
	if !strings.Contains(err.Error(), "one-to-one") {
		t.Fatalf("expected one-to-one credential error, got %v", err)
	}
}

func TestLoadFileRejectsRuleCredential(t *testing.T) {
	dir := t.TempDir()
	policyPath := filepath.Join(dir, "policy.hcl")
	writePolicy(t, policyPath, `
endpoint "https" "github" {
  hosts = ["api.github.com"]
}

credential "bearer_token" "github" {
  endpoint = https.github
  secret = "github-token"
}

rule "github-reads" {
  endpoint = https.github
  verdict = "allow"
  credential = bearer_token.github
}
`)

	_, err := LoadFile(policyPath)
	if err == nil {
		t.Fatal("expected rule-level credential to be rejected")
	}
	if !strings.Contains(err.Error(), "Unsupported argument") || !strings.Contains(err.Error(), "credential") {
		t.Fatalf("expected unsupported credential argument error, got %v", err)
	}
}

func TestLoadFileRejectsMixedEndpointRule(t *testing.T) {
	dir := t.TempDir()
	policyPath := filepath.Join(dir, "policy.hcl")
	writePolicy(t, policyPath, `
endpoint "cidr" "private" {
  cidrs = ["10.0.0.0/8"]
}

endpoint "https" "github" {
  hosts = ["api.github.com"]
}

rule "mixed" {
  endpoints = [cidr.private, https.github]
  verdict = "allow"
}
`)

	if _, err := LoadFile(policyPath); err == nil {
		t.Fatal("expected mixed endpoint kinds to be rejected")
	}
}

func writePolicy(t *testing.T, path string, text string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(text), 0o600); err != nil {
		t.Fatal(err)
	}
}
