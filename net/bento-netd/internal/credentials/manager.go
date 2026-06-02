package credentials

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/nickvan/bentobox/net/bento-netd/internal/gateway/hooks"
)

const (
	kindBearerToken      = "bearer_token"
	kindOpenAICodexOAuth = "openai_codex_oauth"

	openAICodexClientID = "app_EMoamEEZ73f0CkXaXp7hrann"
	openAITokenURL      = "https://auth.openai.com/oauth/token"

	refreshSkew        = 60 * time.Second
	tokenFileSizeLimit = 1 << 20
	oauthBodyLimit     = 64 << 10
)

type Manager struct {
	client         *http.Client
	now            func() time.Time
	openAITokenURL string

	mu    sync.Mutex
	locks map[string]*sync.Mutex
}

func NewManager() *Manager {
	return &Manager{
		client: &http.Client{
			Timeout: 30 * time.Second,
		},
		now:            time.Now,
		openAITokenURL: openAITokenURL,
		locks:          make(map[string]*sync.Mutex),
	}
}

func (m *Manager) Apply(ctx context.Context, req *http.Request, credential *hooks.Credential) error {
	if credential == nil {
		return nil
	}
	switch credential.Kind {
	case kindBearerToken:
		return applyBearerToken(req, credential.Value)
	case kindOpenAICodexOAuth:
		return m.applyOpenAICodexOAuth(ctx, req, credential)
	default:
		return fmt.Errorf("unsupported credential kind %q", credential.Kind)
	}
}

func applyBearerToken(req *http.Request, value string) error {
	if value == "" {
		return fmt.Errorf("bearer token credential is empty")
	}
	req.Header.Del("Authorization")
	req.Header.Set("Authorization", "Bearer "+value)
	return nil
}

func (m *Manager) applyOpenAICodexOAuth(ctx context.Context, req *http.Request, credential *hooks.Credential) error {
	if credential.TokenFile == "" {
		return fmt.Errorf("openai_codex_oauth credential %q is missing token_file", credential.Name)
	}
	lock := m.lockFor(credential.TokenFile)
	lock.Lock()
	defer lock.Unlock()

	token, err := readOpenAICodexTokenFile(credential.TokenFile)
	if err != nil {
		return err
	}
	if token.Kind != "" && token.Kind != kindOpenAICodexOAuth {
		return fmt.Errorf("credential token file %s has kind %q, expected %q", credential.TokenFile, token.Kind, kindOpenAICodexOAuth)
	}
	if token.AccessToken == "" {
		return fmt.Errorf("credential token file %s is missing access_token", credential.TokenFile)
	}
	if m.needsRefresh(token.ExpiresAt) {
		if token.RefreshToken == "" {
			return fmt.Errorf("credential token file %s is expired and missing refresh_token", credential.TokenFile)
		}
		refreshed, err := m.refreshOpenAICodexToken(ctx, token)
		if err != nil {
			return err
		}
		refreshed.CreatedAt = token.CreatedAt
		if refreshed.CreatedAt == "" {
			refreshed.CreatedAt = rfc3339(m.now())
		}
		refreshed.AccountID = token.AccountID
		if refreshed.AccountID == "" {
			refreshed.AccountID = chatGPTAccountID(refreshed.AccessToken)
		}
		if err := writeOpenAICodexTokenFile(credential.TokenFile, refreshed); err != nil {
			return err
		}
		token = refreshed
	}

	accountID := token.AccountID
	if accountID == "" {
		accountID = chatGPTAccountID(token.AccessToken)
	}
	req.Header.Del("Authorization")
	req.Header.Del("ChatGPT-Account-Id")
	req.Header.Set("Authorization", "Bearer "+token.AccessToken)
	if accountID != "" {
		req.Header.Set("ChatGPT-Account-Id", accountID)
	}
	return nil
}

func (m *Manager) lockFor(path string) *sync.Mutex {
	path = filepath.Clean(path)
	m.mu.Lock()
	defer m.mu.Unlock()
	lock := m.locks[path]
	if lock == nil {
		lock = &sync.Mutex{}
		m.locks[path] = lock
	}
	return lock
}

func (m *Manager) needsRefresh(expiresAt string) bool {
	expiry, err := time.Parse(time.RFC3339, expiresAt)
	if err != nil {
		return true
	}
	return !m.now().Add(refreshSkew).Before(expiry)
}

func (m *Manager) refreshOpenAICodexToken(ctx context.Context, current openAICodexTokenFile) (openAICodexTokenFile, error) {
	form := url.Values{}
	form.Set("grant_type", "refresh_token")
	form.Set("refresh_token", current.RefreshToken)
	form.Set("client_id", openAICodexClientID)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, m.openAITokenURL, strings.NewReader(form.Encode()))
	if err != nil {
		return openAICodexTokenFile{}, fmt.Errorf("build OpenAI Codex token refresh request: %w", err)
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.Header.Set("Accept", "application/json")
	resp, err := m.client.Do(req)
	if err != nil {
		return openAICodexTokenFile{}, fmt.Errorf("refresh OpenAI Codex token: %w", err)
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(io.LimitReader(resp.Body, oauthBodyLimit))
	if resp.StatusCode != http.StatusOK {
		return openAICodexTokenFile{}, fmt.Errorf("refresh OpenAI Codex token returned %d: %s", resp.StatusCode, sanitizeResponseBody(body))
	}
	var decoded struct {
		AccessToken  string `json:"access_token"`
		RefreshToken string `json:"refresh_token"`
		ExpiresIn    int64  `json:"expires_in"`
	}
	if err := json.Unmarshal(body, &decoded); err != nil {
		return openAICodexTokenFile{}, fmt.Errorf("decode OpenAI Codex token refresh response: %w", err)
	}
	if decoded.AccessToken == "" {
		return openAICodexTokenFile{}, fmt.Errorf("OpenAI Codex token refresh response is missing access_token")
	}
	if decoded.RefreshToken == "" {
		decoded.RefreshToken = current.RefreshToken
	}
	if decoded.ExpiresIn <= 0 {
		return openAICodexTokenFile{}, fmt.Errorf("OpenAI Codex token refresh response is missing expires_in")
	}
	now := m.now()
	return openAICodexTokenFile{
		Version:      1,
		Kind:         kindOpenAICodexOAuth,
		AccessToken:  decoded.AccessToken,
		RefreshToken: decoded.RefreshToken,
		ExpiresAt:    rfc3339(now.Add(time.Duration(decoded.ExpiresIn) * time.Second)),
		CreatedAt:    current.CreatedAt,
		UpdatedAt:    rfc3339(now),
	}, nil
}

type openAICodexTokenFile struct {
	Version      int    `json:"version"`
	Kind         string `json:"kind"`
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
	ExpiresAt    string `json:"expires_at"`
	AccountID    string `json:"account_id,omitempty"`
	CreatedAt    string `json:"created_at"`
	UpdatedAt    string `json:"updated_at"`
}

func readOpenAICodexTokenFile(path string) (openAICodexTokenFile, error) {
	f, err := os.Open(path)
	if err != nil {
		return openAICodexTokenFile{}, fmt.Errorf("open credential token file %s: %w", path, err)
	}
	defer f.Close()
	var token openAICodexTokenFile
	decoder := json.NewDecoder(io.LimitReader(f, tokenFileSizeLimit))
	if err := decoder.Decode(&token); err != nil {
		return openAICodexTokenFile{}, fmt.Errorf("decode credential token file %s: %w", path, err)
	}
	return token, nil
}

func writeOpenAICodexTokenFile(path string, token openAICodexTokenFile) error {
	body, err := json.MarshalIndent(token, "", "  ")
	if err != nil {
		return fmt.Errorf("encode credential token file %s: %w", path, err)
	}
	body = append(body, '\n')
	dir := filepath.Dir(path)
	base := filepath.Base(path)
	tmpPath := filepath.Join(dir, fmt.Sprintf(".%s.tmp.%d", base, os.Getpid()))
	if err := os.WriteFile(tmpPath, body, 0o600); err != nil {
		return fmt.Errorf("write credential token file %s: %w", tmpPath, err)
	}
	if err := os.Chmod(tmpPath, 0o600); err != nil {
		_ = os.Remove(tmpPath)
		return fmt.Errorf("secure credential token file %s: %w", tmpPath, err)
	}
	if err := os.Rename(tmpPath, path); err != nil {
		_ = os.Remove(tmpPath)
		return fmt.Errorf("replace credential token file %s: %w", path, err)
	}
	if err := os.Chmod(path, 0o600); err != nil {
		return fmt.Errorf("secure credential token file %s: %w", path, err)
	}
	return nil
}

func chatGPTAccountID(accessToken string) string {
	parts := strings.Split(accessToken, ".")
	if len(parts) < 2 {
		return ""
	}
	payload, err := decodeJWTPayload(parts[1])
	if err != nil {
		return ""
	}
	var claims map[string]any
	if err := json.Unmarshal(payload, &claims); err != nil {
		return ""
	}
	if value, ok := claims["chatgpt_account_id"].(string); ok {
		return value
	}
	nested, ok := claims["https://api.openai.com/auth"].(map[string]any)
	if !ok {
		return ""
	}
	value, _ := nested["chatgpt_account_id"].(string)
	return value
}

func decodeJWTPayload(payload string) ([]byte, error) {
	decoded, err := base64.RawURLEncoding.DecodeString(payload)
	if err == nil {
		return decoded, nil
	}
	if missing := len(payload) % 4; missing != 0 {
		payload += strings.Repeat("=", 4-missing)
	}
	return base64.URLEncoding.DecodeString(payload)
}

func rfc3339(t time.Time) string {
	return t.UTC().Format(time.RFC3339)
}

func sanitizeResponseBody(body []byte) string {
	body = bytes.TrimSpace(body)
	if len(body) == 0 {
		return "<empty>"
	}
	if len(body) > 512 {
		return string(body[:512]) + "..."
	}
	return string(body)
}
