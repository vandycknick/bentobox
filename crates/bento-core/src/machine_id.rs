use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MachineId(Ulid);

impl MachineId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }

    pub fn from_ulid(ulid: Ulid) -> Self {
        Self(ulid)
    }

    pub fn as_ulid(&self) -> Ulid {
        self.0
    }
}

impl Default for MachineId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for MachineId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl From<Ulid> for MachineId {
    fn from(value: Ulid) -> Self {
        Self::from_ulid(value)
    }
}

impl From<MachineId> for Ulid {
    fn from(value: MachineId) -> Self {
        value.0
    }
}

impl FromStr for MachineId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::MachineId;

    #[test]
    fn machine_id_round_trips_through_string() {
        let id = MachineId::new();
        let parsed: MachineId = id.to_string().parse().expect("parse machine id");

        assert_eq!(parsed, id);
    }
}
