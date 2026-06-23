package hooks

import (
	"context"
	"net"
	"strings"
	"testing"

	"github.com/vandycknick/bentobox/net/bento-netd/internal/policy"
)

func TestPolicyHookCarriesL4MatchMetadata(t *testing.T) {
	compiled, err := policy.LoadReader("policy.hcl", strings.NewReader(`
settings {
  default_action = "deny"
}

endpoint "ip" "app" {
  destination = ["10.0.0.0/8"]
  protocol = "tcp"
  ports = ["8443-9443"]
}

rule "allow-app" {
  endpoint = ip.app
  verdict = "allow"
}
`))
	if err != nil {
		t.Fatalf("LoadReader returned error: %v", err)
	}

	decision, err := NewPolicyHook(compiled).Decide(context.Background(), Flow{
		Protocol: "tcp",
		SourceIP: net.ParseIP("192.168.127.2"),
		DestIP:   net.ParseIP("10.1.2.3"),
		DestPort: 9000,
	})
	if err != nil {
		t.Fatalf("Decide returned error: %v", err)
	}
	if decision.Action != RouteAllowDirect || decision.RuleName != "allow-app" {
		t.Fatalf("expected allow-app route decision, got %#v", decision)
	}
	want := L4Match{EndpointProtocol: "tcp", DestPort: 9000, PortRange: PortRange{Start: 8443, End: 9443}, Kind: L4MatchRange}
	if decision.MatchedL4 == nil {
		t.Fatalf("expected l4 match %#v, got nil", want)
	}
	if *decision.MatchedL4 != want {
		t.Fatalf("expected l4 match %#v, got %#v", want, *decision.MatchedL4)
	}
}
