package policy

import (
	"fmt"

	"github.com/hashicorp/hcl/v2"
	"github.com/hashicorp/hcl/v2/gohcl"
)

func decodeStringAttr(attr *hcl.Attribute) (string, error) {
	var value string
	if diagnostics := gohcl.DecodeExpression(attr.Expr, nil, &value); diagnostics.HasErrors() {
		return "", fmt.Errorf("%s", diagnostics.Error())
	}
	return value, nil
}

func decodeIntAttr(attr *hcl.Attribute) (int, error) {
	var value int
	if diagnostics := gohcl.DecodeExpression(attr.Expr, nil, &value); diagnostics.HasErrors() {
		return 0, fmt.Errorf("%s", diagnostics.Error())
	}
	return value, nil
}

func decodeBoolAttr(attr *hcl.Attribute) (bool, error) {
	var value bool
	if diagnostics := gohcl.DecodeExpression(attr.Expr, nil, &value); diagnostics.HasErrors() {
		return false, fmt.Errorf("%s", diagnostics.Error())
	}
	return value, nil
}

func decodeRefAttr(attr *hcl.Attribute) (Ref, error) {
	traversal, diagnostics := hcl.AbsTraversalForExpr(attr.Expr)
	if diagnostics.HasErrors() {
		return Ref{}, fmt.Errorf("expected reference like https.github: %s", diagnostics.Error())
	}
	return refFromTraversal(traversal)
}

func decodeRefListAttr(attr *hcl.Attribute) ([]Ref, error) {
	variables := attr.Expr.Variables()
	if len(variables) == 0 {
		return nil, fmt.Errorf("expected at least one reference")
	}
	refs := make([]Ref, 0, len(variables))
	for _, variable := range variables {
		ref, err := refFromTraversal(variable)
		if err != nil {
			return nil, err
		}
		refs = append(refs, ref)
	}
	return refs, nil
}

func refFromTraversal(traversal hcl.Traversal) (Ref, error) {
	if len(traversal) != 2 {
		return Ref{}, fmt.Errorf("expected two-part reference like https.github")
	}
	root, ok := traversal[0].(hcl.TraverseRoot)
	if !ok {
		return Ref{}, fmt.Errorf("expected reference root")
	}
	attr, ok := traversal[1].(hcl.TraverseAttr)
	if !ok {
		return Ref{}, fmt.Errorf("expected reference attribute")
	}
	if root.Name == "" || attr.Name == "" {
		return Ref{}, fmt.Errorf("reference parts must not be empty")
	}
	return Ref{Kind: root.Name, Name: attr.Name}, nil
}

func parseTerminalAction(value string) (Action, error) {
	switch Action(value) {
	case "", ActionAllow:
		return ActionAllow, nil
	case ActionDeny:
		return ActionDeny, nil
	default:
		return "", fmt.Errorf("invalid action %q, expected allow or deny", value)
	}
}

func parseRuleAction(value string) (Action, error) {
	switch Action(value) {
	case ActionAllow, ActionDeny, ActionAudit:
		return Action(value), nil
	default:
		return "", fmt.Errorf("invalid verdict %q, expected allow, deny, or audit", value)
	}
}
