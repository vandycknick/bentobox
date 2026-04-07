use std::str::FromStr;

use bento_core::{looks_like_id_prefix, MachineId};

use crate::LibVmError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MachineRef {
    Id(MachineId),
    IdPrefix(String),
    Name(String),
}

impl MachineRef {
    pub fn parse(input: impl Into<String>) -> Result<Self, LibVmError> {
        let input = input.into();
        if let Ok(id) = MachineId::from_str(&input) {
            return Ok(Self::Id(id));
        }

        if looks_like_id_prefix(&input) {
            return Ok(Self::IdPrefix(input.to_lowercase()));
        }

        validate_machine_name(&input)?;
        Ok(Self::Name(input))
    }
}

pub(crate) fn validate_machine_name(name: &str) -> Result<(), LibVmError> {
    if name.is_empty() {
        return Err(LibVmError::InvalidMachineName {
            name: name.to_string(),
            reason: "name cannot be empty".to_string(),
        });
    }

    if name.starts_with('-') {
        return Err(LibVmError::InvalidMachineName {
            name: name.to_string(),
            reason: "name cannot start with '-'".to_string(),
        });
    }

    if let Some(ch) = name
        .chars()
        .find(|ch| !matches!(ch, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.'))
    {
        return Err(LibVmError::InvalidMachineName {
            name: name.to_string(),
            reason: format!("unsupported character {ch:?}"),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::MachineRef;
    use bento_core::MachineId;

    #[test]
    fn parse_treats_full_uuid_as_machine_id() {
        let id = MachineId::new();
        let machine_ref = MachineRef::parse(id.to_string()).expect("parse machine ref");

        assert_eq!(machine_ref, MachineRef::Id(id));
    }

    #[test]
    fn parse_treats_hex_prefix_as_id_prefix() {
        let machine_ref = MachineRef::parse("a1b2c3d4").expect("parse machine ref");

        assert_eq!(machine_ref, MachineRef::IdPrefix("a1b2c3d4".to_string()));
    }

    #[test]
    fn parse_treats_non_hex_as_name() {
        let machine_ref = MachineRef::parse("devbox").expect("parse machine ref");

        assert_eq!(machine_ref, MachineRef::Name("devbox".to_string()));
    }

    #[test]
    fn parse_rejects_invalid_name() {
        let err = MachineRef::parse("bad/name").expect_err("invalid name should fail");

        assert!(err.to_string().contains("unsupported character"));
    }

    #[test]
    fn parse_short_hex_is_name_not_prefix() {
        // "ab" is only 2 chars, too short to be an ID prefix
        let machine_ref = MachineRef::parse("ab").expect("parse machine ref");
        assert_eq!(machine_ref, MachineRef::Name("ab".to_string()));
    }
}
