package hooks

import (
	"context"
	"net"
	"net/http"
)

type RouteAction string

const (
	RouteAllowDirect RouteAction = "allow_direct"
	RouteDeny        RouteAction = "deny"
)

type Flow struct {
	Protocol    string
	SourceIP    net.IP
	SourcePort  uint16
	DestIP      net.IP
	DestPort    uint16
	VMID        string
	NetworkID   string
	ProfileName string
}

type HTTPRequest struct {
	Flow   Flow
	Host   string
	Method string
	Path   string
	Header http.Header
}

type AuditEvent struct {
	RuleName     string
	Reason       string
	EndpointKind string
	EndpointName string
	Layer        string
}

type Credential struct {
	Kind   string
	Name   string
	Secret string
}

type RouteDecision struct {
	Action       RouteAction
	Reason       string
	RuleName     string
	EndpointKind string
	EndpointName string
	AuditEvents  []AuditEvent
	Credential   *Credential
}

type Hook interface {
	Decide(ctx context.Context, flow Flow) (RouteDecision, error)
}
