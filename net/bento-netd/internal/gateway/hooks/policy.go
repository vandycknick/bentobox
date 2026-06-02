package hooks

import (
	"context"

	"github.com/nickvan/bentobox/net/bento-netd/internal/policy"
)

type PolicyHook struct {
	policy *policy.Policy
}

func NewPolicyHook(compiled *policy.Policy) *PolicyHook {
	if compiled == nil {
		compiled = policy.Default()
	}
	return &PolicyHook{policy: compiled}
}

func (h *PolicyHook) Decide(_ context.Context, flow Flow) (RouteDecision, error) {
	decision := h.policy.EvaluateFlow(policy.Flow{
		Protocol:   flow.Protocol,
		SourceIP:   flow.SourceIP,
		SourcePort: flow.SourcePort,
		DestIP:     flow.DestIP,
		DestPort:   flow.DestPort,
	})
	return routeDecisionFromPolicy(decision, "l3_l4"), nil
}

func (h *PolicyHook) HasHTTPS() bool {
	return h.policy.HasHTTPS()
}

func (h *PolicyHook) MatchHTTPSHost(host string) bool {
	return h.policy.MatchHTTPSHost(host)
}

func (h *PolicyHook) DecideHTTP(_ context.Context, request HTTPRequest) (RouteDecision, error) {
	decision := h.policy.EvaluateHTTP(policy.HTTPRequest{
		Flow: policy.Flow{
			Protocol:   request.Flow.Protocol,
			SourceIP:   request.Flow.SourceIP,
			SourcePort: request.Flow.SourcePort,
			DestIP:     request.Flow.DestIP,
			DestPort:   request.Flow.DestPort,
		},
		Host:   request.Host,
		Method: request.Method,
		Path:   request.Path,
		Header: request.Header,
	})
	return routeDecisionFromPolicy(decision, "https"), nil
}

func routeDecisionFromPolicy(decision policy.Decision, layer string) RouteDecision {
	converted := RouteDecision{
		Action:       routeActionFromPolicy(decision.Action),
		Reason:       decision.Reason,
		RuleName:     decision.RuleName,
		EndpointKind: decision.EndpointKind,
		EndpointName: decision.EndpointName,
		AuditEvents:  make([]AuditEvent, 0, len(decision.Audits)),
	}
	for _, event := range decision.Audits {
		converted.AuditEvents = append(converted.AuditEvents, AuditEvent{
			RuleName:     event.RuleName,
			Reason:       event.Reason,
			EndpointKind: event.EndpointKind,
			EndpointName: event.EndpointName,
			Layer:        layer,
		})
	}
	if decision.Credential != nil {
		converted.Credential = &Credential{
			Kind:      decision.Credential.Kind,
			Name:      decision.Credential.Name,
			ValueFile: decision.Credential.ValueFile,
			TokenFile: decision.Credential.TokenFile,
			Value:     decision.Credential.Value,
		}
	}
	return converted
}

func routeActionFromPolicy(action policy.Action) RouteAction {
	switch action {
	case policy.ActionDeny:
		return RouteDeny
	default:
		return RouteAllowDirect
	}
}
