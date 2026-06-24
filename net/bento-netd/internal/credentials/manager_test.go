package credentials

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/vandycknick/bentobox/net/bento-netd/internal/gateway/hooks"
)

func TestManagerApplyIsNoopUntilCredentialRuntimeExists(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "https://api.example.test", nil)
	req.Header.Set("Authorization", "Bearer guest-token")
	manager := NewManager(nil)

	err := manager.Apply(context.Background(), req, &hooks.Credential{Kind: "bearer_token", Name: "api"})
	if err != nil {
		t.Fatalf("Apply returned error: %v", err)
	}
	if got := req.Header.Get("Authorization"); got != "Bearer guest-token" {
		t.Fatalf("credential runtime should be a no-op before ticket 92, got Authorization %q", got)
	}

	err = manager.Apply(context.Background(), req, &hooks.Credential{Kind: "unknown_kind", Name: "api"})
	if err != nil {
		t.Fatalf("unsupported credential kinds should be a no-op before ticket 92, got %v", err)
	}
}

func TestFailureReasonUsesClassifiedApplyError(t *testing.T) {
	err := applyError(ReasonRefresh, "refresh failed")
	if got := FailureReason(err); got != ReasonRefresh {
		t.Fatalf("expected %q, got %q", ReasonRefresh, got)
	}
	var applyErr *ApplyError
	if !errors.As(err, &applyErr) {
		t.Fatalf("expected ApplyError, got %T", err)
	}
	if got := FailureReason(errors.New("plain")); got != ReasonInjection {
		t.Fatalf("expected unclassified errors to map to %q, got %q", ReasonInjection, got)
	}
}
