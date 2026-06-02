package credentials

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/nickvan/bentobox/net/bento-netd/internal/gateway/hooks"
)

func TestManagerInjectsBearerToken(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "https://api.example.test", nil)
	req.Header.Set("Authorization", "Bearer guest-token")
	manager := NewManager()

	err := manager.Apply(context.Background(), req, &hooks.Credential{
		Kind:  kindBearerToken,
		Name:  "api",
		Value: "host-token",
	})
	if err != nil {
		t.Fatalf("Apply returned error: %v", err)
	}
	if got := req.Header.Get("Authorization"); got != "Bearer host-token" {
		t.Fatalf("expected bearer token injection, got %q", got)
	}
}

func TestManagerInjectsOpenAICodexOAuthHeaders(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "codex.json")
	accessToken := fakeJWT(t, map[string]any{"chatgpt_account_id": "acct_123"})
	writeTokenFileForTest(t, path, openAICodexTokenFile{
		Version:      1,
		Kind:         kindOpenAICodexOAuth,
		AccessToken:  accessToken,
		RefreshToken: "refresh-token",
		ExpiresAt:    rfc3339(time.Now().Add(time.Hour)),
		CreatedAt:    rfc3339(time.Now()),
		UpdatedAt:    rfc3339(time.Now()),
	})
	req := httptest.NewRequest(http.MethodPost, "https://chatgpt.com/backend-api/codex/responses", nil)
	req.Header.Set("Authorization", "Bearer guest-token")
	req.Header.Set("ChatGPT-Account-Id", "guest-account")

	manager := NewManager()
	err := manager.Apply(context.Background(), req, &hooks.Credential{
		Kind:      kindOpenAICodexOAuth,
		Name:      "codex",
		TokenFile: path,
	})
	if err != nil {
		t.Fatalf("Apply returned error: %v", err)
	}
	if got := req.Header.Get("Authorization"); got != "Bearer "+accessToken {
		t.Fatalf("expected OpenAI access token injection, got %q", got)
	}
	if got := req.Header.Get("ChatGPT-Account-Id"); got != "acct_123" {
		t.Fatalf("expected ChatGPT account id injection, got %q", got)
	}
}

func TestManagerRefreshesOpenAICodexOAuthTokenFile(t *testing.T) {
	fixedNow := time.Date(2026, 6, 2, 12, 0, 0, 0, time.UTC)
	dir := t.TempDir()
	path := filepath.Join(dir, "codex.json")
	oldAccessToken := fakeJWT(t, map[string]any{"chatgpt_account_id": "old_account"})
	newAccessToken := fakeJWT(t, map[string]any{
		"https://api.openai.com/auth": map[string]any{"chatgpt_account_id": "new_account"},
	})
	writeTokenFileForTest(t, path, openAICodexTokenFile{
		Version:      1,
		Kind:         kindOpenAICodexOAuth,
		AccessToken:  oldAccessToken,
		RefreshToken: "old-refresh-token",
		ExpiresAt:    rfc3339(fixedNow.Add(-time.Hour)),
		CreatedAt:    rfc3339(fixedNow.Add(-24 * time.Hour)),
		UpdatedAt:    rfc3339(fixedNow.Add(-24 * time.Hour)),
	})
	var refreshRequests int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		refreshRequests++
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST refresh, got %s", r.Method)
		}
		body, err := io.ReadAll(r.Body)
		if err != nil {
			t.Fatalf("read refresh body: %v", err)
		}
		values, err := url.ParseQuery(string(body))
		if err != nil {
			t.Fatalf("parse refresh body: %v", err)
		}
		if got := values.Get("grant_type"); got != "refresh_token" {
			t.Fatalf("expected refresh_token grant, got %q", got)
		}
		if got := values.Get("refresh_token"); got != "old-refresh-token" {
			t.Fatalf("expected old refresh token, got %q", got)
		}
		if got := values.Get("client_id"); got != openAICodexClientID {
			t.Fatalf("expected OpenAI Codex client id, got %q", got)
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"access_token":"` + newAccessToken + `","refresh_token":"new-refresh-token","expires_in":3600}`))
	}))
	defer server.Close()

	manager := NewManager()
	manager.client = server.Client()
	manager.openAITokenURL = server.URL
	manager.now = func() time.Time { return fixedNow }
	req := httptest.NewRequest(http.MethodPost, "https://chatgpt.com/backend-api/codex/responses", nil)

	err := manager.Apply(context.Background(), req, &hooks.Credential{
		Kind:      kindOpenAICodexOAuth,
		Name:      "codex",
		TokenFile: path,
	})
	if err != nil {
		t.Fatalf("Apply returned error: %v", err)
	}
	if refreshRequests != 1 {
		t.Fatalf("expected one refresh request, got %d", refreshRequests)
	}
	if got := req.Header.Get("Authorization"); got != "Bearer "+newAccessToken {
		t.Fatalf("expected refreshed OpenAI access token injection, got %q", got)
	}
	if got := req.Header.Get("ChatGPT-Account-Id"); got != "new_account" {
		t.Fatalf("expected refreshed ChatGPT account id injection, got %q", got)
	}
	refreshed := readTokenFileForTest(t, path)
	if refreshed.AccessToken != newAccessToken {
		t.Fatalf("expected refreshed access token persisted, got %q", refreshed.AccessToken)
	}
	if refreshed.RefreshToken != "new-refresh-token" {
		t.Fatalf("expected refreshed refresh token persisted, got %q", refreshed.RefreshToken)
	}
	if refreshed.ExpiresAt != rfc3339(fixedNow.Add(time.Hour)) {
		t.Fatalf("expected refreshed expiry, got %q", refreshed.ExpiresAt)
	}
	if refreshed.AccountID != "new_account" {
		t.Fatalf("expected derived account id persisted, got %q", refreshed.AccountID)
	}
}

func TestChatGPTAccountIDReadsNestedClaim(t *testing.T) {
	token := fakeJWT(t, map[string]any{
		"https://api.openai.com/auth": map[string]any{"chatgpt_account_id": "acct_nested"},
	})
	if got := chatGPTAccountID(token); got != "acct_nested" {
		t.Fatalf("expected nested account id, got %q", got)
	}
}

func writeTokenFileForTest(t *testing.T, path string, token openAICodexTokenFile) {
	t.Helper()
	if err := writeOpenAICodexTokenFile(path, token); err != nil {
		t.Fatalf("write token file: %v", err)
	}
}

func readTokenFileForTest(t *testing.T, path string) openAICodexTokenFile {
	t.Helper()
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read token file: %v", err)
	}
	var token openAICodexTokenFile
	if err := json.Unmarshal(raw, &token); err != nil {
		t.Fatalf("decode token file: %v", err)
	}
	return token
}

func fakeJWT(t *testing.T, claims map[string]any) string {
	t.Helper()
	header, err := json.Marshal(map[string]any{"alg": "none", "typ": "JWT"})
	if err != nil {
		t.Fatalf("encode jwt header: %v", err)
	}
	payload, err := json.Marshal(claims)
	if err != nil {
		t.Fatalf("encode jwt payload: %v", err)
	}
	return base64.RawURLEncoding.EncodeToString(header) + "." + base64.RawURLEncoding.EncodeToString(payload) + ".signature"
}
