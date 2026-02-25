use eyre::Context;
use serde::Serialize;

use crate::cidata_iso9660::{write_cidata_iso, CidataEntry};
use crate::host_user::HostUser;
use crate::instance::{resolve_mount_location, Instance, InstanceFile};

pub fn build_cidata_iso(
    inst: &Instance,
    host_user: &HostUser,
    ssh_public_key: &str,
) -> eyre::Result<()> {
    let user_data = render_user_data(inst, host_user, ssh_public_key)?;
    let meta_data = render_meta_data(inst)?;
    let _network_config = render_network_config()?;
    let iso_path = inst.file(InstanceFile::CidataIso);
    let files = vec![
        CidataEntry {
            name: "user-data".to_string(),
            contents: user_data.into_bytes(),
        },
        CidataEntry {
            name: "meta-data".to_string(),
            contents: meta_data.into_bytes(),
        },
        // TODO: only add this when needed
        //
        // CidataEntry {
        //     name: "network-config".to_string(),
        //     contents: network_config.into_bytes(),
        // },
    ];

    write_cidata_iso(&iso_path, "CIDATA", &files)
        .with_context(|| format!("build cidata ISO at {}", iso_path.display()))?;

    Ok(())
}

#[derive(Serialize)]
struct CloudConfig {
    users: Vec<CloudUser>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mounts: Vec<[String; 6]>,
}

#[derive(Serialize)]
struct CloudUser {
    name: String,
    uid: u32,
    gecos: String,
    homedir: String,
    shell: String,
    sudo: String,
    lock_passwd: bool,
    ssh_authorized_keys: Vec<String>,
}

#[derive(Serialize)]
struct MetaData {
    #[serde(rename = "instance-id")]
    instance_id: String,
    #[serde(rename = "local-hostname")]
    local_hostname: String,
}

#[derive(Serialize)]
struct NetworkConfigV2 {
    version: u8,
    ethernets: std::collections::BTreeMap<String, EthernetConfigV2>,
}

#[derive(Serialize)]
struct EthernetConfigV2 {
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    match_cfg: Option<MatchByName>,
    dhcp4: bool,
    dhcp6: bool,
}

#[derive(Serialize)]
struct MatchByName {
    name: String,
}

fn render_user_data(
    inst: &Instance,
    host_user: &HostUser,
    ssh_public_key: &str,
) -> eyre::Result<String> {
    let user = CloudUser {
        name: host_user.name.clone(),
        uid: host_user.uid,
        gecos: host_user.gecos.clone(),
        homedir: format!("/home/{}", host_user.name),
        shell: "/bin/bash".to_string(),
        sudo: "ALL=(ALL) NOPASSWD:ALL".to_string(),
        lock_passwd: true,
        ssh_authorized_keys: vec![ssh_public_key.trim().to_string()],
    };

    let mut mounts = Vec::with_capacity(inst.config.mounts.len());
    for (index, mount) in inst.config.mounts.iter().enumerate() {
        let location = resolve_mount_location(&mount.location)
            .map_err(|reason| eyre::eyre!("resolve mount location failed: {reason}"))?;
        let options = if mount.writable {
            "rw,nofail"
        } else {
            "ro,nofail"
        };
        mounts.push([
            format!("mount{index}"),
            location.to_string_lossy().to_string(),
            "virtiofs".to_string(),
            options.to_string(),
            "0".to_string(),
            "0".to_string(),
        ]);
    }

    let cloud_config = CloudConfig {
        users: vec![user],
        mounts,
    };
    let mut yaml = String::from("#cloud-config\n");
    yaml.push_str(
        &serde_yaml_ng::to_string(&cloud_config).context("serialize cloud-init user-data")?,
    );
    Ok(yaml)
}

fn render_network_config() -> eyre::Result<String> {
    let mut ethernets = std::collections::BTreeMap::new();
    ethernets.insert(
        "default".to_string(),
        EthernetConfigV2 {
            match_cfg: Some(MatchByName {
                name: "e*".to_string(),
            }),
            dhcp4: true,
            dhcp6: false,
        },
    );

    let cfg = NetworkConfigV2 {
        version: 2,
        ethernets,
    };
    serde_yaml_ng::to_string(&cfg).context("serialize cloud-init network-config")
}

fn render_meta_data(inst: &Instance) -> eyre::Result<String> {
    let metadata = MetaData {
        instance_id: format!("bento-{}", inst.name),
        local_hostname: inst.name.clone(),
    };
    serde_yaml_ng::to_string(&metadata).context("serialize cloud-init meta-data")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::{InstanceConfig, MountConfig};
    use std::path::PathBuf;

    fn instance_with_mounts(mounts: Vec<MountConfig>) -> Instance {
        let mut config = InstanceConfig::new();
        config.mounts = mounts;
        Instance::new("vm1".to_string(), PathBuf::from("/tmp/vm1"), config)
    }

    #[test]
    fn user_data_contains_expected_user_fields() {
        let host_user = HostUser {
            name: "nickvd".to_string(),
            uid: 504,
            gecos: "Nick Van Dyck".to_string(),
        };
        let inst = instance_with_mounts(Vec::new());

        let user_data = render_user_data(
            &inst,
            &host_user,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestValue nickvd@host",
        )
        .expect("render user-data");

        assert!(user_data.starts_with("#cloud-config\n"));
        assert!(user_data.contains("name: nickvd"));
        assert!(user_data.contains("uid: 504"));
        assert!(user_data.contains("homedir: /home/nickvd"));
        assert!(user_data.contains("ssh_authorized_keys"));
        assert!(!user_data.contains("network:"));
    }

    #[test]
    fn user_data_contains_mount_rows_with_indexed_tags() {
        let host_user = HostUser {
            name: "nickvd".to_string(),
            uid: 504,
            gecos: "Nick Van Dyck".to_string(),
        };
        let inst = instance_with_mounts(vec![
            MountConfig {
                location: PathBuf::from("/Users/nickvd"),
                writable: true,
            },
            MountConfig {
                location: PathBuf::from("/tmp/lima"),
                writable: false,
            },
        ]);

        let user_data = render_user_data(
            &inst,
            &host_user,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestValue nickvd@host",
        )
        .expect("render user-data");

        assert!(user_data.contains("mounts:"));
        assert!(user_data.contains("- mount0"));
        assert!(user_data.contains("- mount1"));
        assert!(user_data.contains("- /Users/nickvd"));
        assert!(user_data.contains("- /tmp/lima"));
        assert!(user_data.contains("- rw,nofail"));
        assert!(user_data.contains("- ro,nofail"));
    }

    #[test]
    fn user_data_expands_tilde_mount_location() {
        let home = std::env::var_os("HOME").expect("HOME should be set");
        let home = PathBuf::from(home);

        let host_user = HostUser {
            name: "nickvd".to_string(),
            uid: 504,
            gecos: "Nick Van Dyck".to_string(),
        };
        let inst = instance_with_mounts(vec![MountConfig {
            location: PathBuf::from("~"),
            writable: true,
        }]);

        let user_data = render_user_data(
            &inst,
            &host_user,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestValue nickvd@host",
        )
        .expect("render user-data");

        assert!(user_data.contains("- mount0"));
        assert!(user_data.contains(&format!("- {}", home.display())));
    }
}
