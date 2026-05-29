use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkPolicySpec {
    pub default_action: PolicyAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_log: Option<AuditLogSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cidr_rules: Vec<CidrRuleSpec>,
}

impl NetworkPolicySpec {
    pub fn required_features(&self) -> BTreeSet<NetworkPolicyFeature> {
        let mut features = BTreeSet::new();
        if !self.cidr_rules.is_empty() {
            features.insert(NetworkPolicyFeature::CidrRules);
        }
        features
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditLogSpec {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CidrRuleSpec {
    pub name: String,
    pub action: PolicyAction,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocols: Vec<NetworkProtocol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_cidrs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dest_cidrs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkProtocol {
    Tcp,
    Udp,
    Icmp,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NetworkPolicyFeature {
    CidrRules,
}
