package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestParseUsesPolicyAuditLogSetting(t *testing.T) {
	dir := t.TempDir()
	policyPath := filepath.Join(dir, "policy.hcl")
	writeConfigPolicy(t, policyPath, `
settings {
  audit_log = "/tmp/bento-audit.jsonl"
}

endpoint "cidr" "private" {
  cidrs = ["10.0.0.0/8"]
}

rule "audit-private" {
  endpoint = cidr.private
  verdict = "audit"
}
`)

	cfg, err := Parse([]string{
		"--listen-vfkit", "unixgram://" + filepath.Join(dir, "net.sock"),
		"--policy-file", policyPath,
	})
	if err != nil {
		t.Fatalf("Parse returned error: %v", err)
	}
	if cfg.AuditLog != "/tmp/bento-audit.jsonl" {
		t.Fatalf("expected policy audit log, got %q", cfg.AuditLog)
	}
}

func TestParseRequiresTLSCAForHTTPSEndpoints(t *testing.T) {
	dir := t.TempDir()
	policyPath := filepath.Join(dir, "policy.hcl")
	writeConfigPolicy(t, policyPath, `
endpoint "https" "github" {
  hosts = ["api.github.com"]
}

rule "audit-github" {
  endpoint = https.github
  verdict = "audit"
}
`)

	_, err := Parse([]string{
		"--listen-vfkit", "unixgram://" + filepath.Join(dir, "net.sock"),
		"--policy-file", policyPath,
	})
	if err == nil {
		t.Fatal("expected missing CA material to be rejected")
	}
}

func TestParseRequiresSecretStoreForCredentials(t *testing.T) {
	dir := t.TempDir()
	policyPath := filepath.Join(dir, "policy.hcl")
	writeConfigPolicy(t, policyPath, `
endpoint "https" "github" {
  hosts = ["api.github.com"]
}

credential "bearer_token" "github" {
  endpoint = https.github
  secret = "github-token"
}

rule "allow-github" {
  endpoint = https.github
  verdict = "allow"
}
`)

	_, err := Parse([]string{
		"--listen-vfkit", "unixgram://" + filepath.Join(dir, "net.sock"),
		"--policy-file", policyPath,
		"--tls-ca-cert", filepath.Join(dir, "ca.pem"),
		"--tls-ca-key", filepath.Join(dir, "ca-key.pem"),
	})
	if err == nil {
		t.Fatal("expected missing secret store to be rejected")
	}
}

func writeConfigPolicy(t *testing.T, path string, text string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(text), 0o600); err != nil {
		t.Fatal(err)
	}
}
