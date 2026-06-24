package credentials

import (
	"context"
	"errors"
	"fmt"
	"net/http"

	"github.com/vandycknick/bentobox/net/bento-netd/internal/gateway/hooks"
	"github.com/vandycknick/bentobox/net/bento-netd/internal/secrets"
)

const (
	ReasonSecret    = "credential_secret_error"
	ReasonRefresh   = "credential_refresh_error"
	ReasonSigning   = "credential_signing_error"
	ReasonInjection = "credential_injection_error"
)

type ApplyError struct {
	Reason string
	Err    error
}

func (e *ApplyError) Error() string {
	if e == nil {
		return ""
	}
	if e.Err == nil {
		return e.Reason
	}
	return e.Reason + ": " + e.Err.Error()
}

func (e *ApplyError) Unwrap() error {
	if e == nil {
		return nil
	}
	return e.Err
}

func FailureReason(err error) string {
	var applyErr *ApplyError
	if errors.As(err, &applyErr) && applyErr.Reason != "" {
		return applyErr.Reason
	}
	return ReasonInjection
}

func applyError(reason string, format string, args ...any) error {
	return &ApplyError{Reason: reason, Err: fmt.Errorf(format, args...)}
}

type Manager struct{}

func NewManager(_ secrets.Store) *Manager {
	return &Manager{}
}

func (m *Manager) Apply(_ context.Context, _ *http.Request, _ *hooks.Credential) error {
	return nil
}
