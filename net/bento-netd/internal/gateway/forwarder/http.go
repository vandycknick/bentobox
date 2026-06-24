package forwarder

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"

	"github.com/vandycknick/bentobox/net/bento-netd/internal/gateway/hooks"
	"github.com/vandycknick/bentobox/net/bento-netd/internal/gateway/router"
)

type HTTPProxy struct {
	route *router.Router
}

func NewHTTPProxy(route *router.Router) *HTTPProxy {
	if route == nil || !route.HasHTTP() {
		return nil
	}
	return &HTTPProxy{route: route}
}

func (p *HTTPProxy) ShouldHandle(flow hooks.Flow, decision hooks.RouteDecision) bool {
	return p != nil && decision.Action == hooks.RouteClassify && p.route.ShouldInterceptHTTP(flow.DestPort)
}

func (p *HTTPProxy) Handle(ctx context.Context, inbound net.Conn, flow hooks.Flow, target string) error {
	defer inbound.Close()

	reader := bufio.NewReader(inbound)
	for {
		req, err := http.ReadRequest(reader)
		if errors.Is(err, io.EOF) {
			return nil
		}
		if err != nil {
			return err
		}
		if req.Host == "" {
			_ = req.Body.Close()
			return writeHTTPStatus(inbound, http.StatusBadRequest, "missing_host")
		}
		if p.route.MatchHTTPHost(req.Host) && !p.route.MatchHTTPHostForPort(req.Host, flow.DestPort) {
			_ = req.Body.Close()
			return writeHTTPStatus(inbound, http.StatusMisdirectedRequest, "host_mismatch")
		}

		decision, err := p.route.DecideHTTP(ctx, hooks.HTTPRequest{
			Flow:         flow,
			EndpointKind: "http",
			Host:         req.Host,
			Method:       req.Method,
			Path:         requestPath(req),
			Query:        req.URL.RawQuery,
			Header:       req.Header.Clone(),
		})
		if err != nil {
			_ = req.Body.Close()
			return err
		}
		if decision.Action == hooks.RouteDeny {
			_ = req.Body.Close()
			return writeDeny(inbound, decision.Reason)
		}

		upgrade := isWebSocketUpgrade(req)
		if err := forwardHTTPFamilyRequest(ctx, inbound, reader, req, "http", req.Host, nil, decision.Credential, func() (net.Conn, error) {
			return net.Dial("tcp", target)
		}); err != nil {
			return err
		}
		if upgrade || req.Close {
			return nil
		}
	}
}
func requestPath(req *http.Request) string {
	path := req.URL.Path
	if path == "" {
		return "/"
	}
	return path
}

func writeDeny(conn net.Conn, reason string) error {
	if reason == "" {
		reason = "request denied by network policy"
	}
	return writeHTTPStatus(conn, statusForReason(reason), reason)
}

func writeHTTPStatus(conn net.Conn, status int, body string) error {
	if body == "" {
		body = http.StatusText(status)
	}
	_, err := fmt.Fprintf(conn, "HTTP/1.1 %d %s\r\nConnection: close\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: %d\r\n\r\n%s", status, http.StatusText(status), len(body), body)
	return err
}
