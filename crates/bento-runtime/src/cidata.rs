use eyre::Context;
use serde::Serialize;

use crate::cidata_iso9660::{write_cidata_iso, CidataEntry};
use crate::host_user::HostUser;
use crate::instance::{Instance, InstanceFile};

pub fn build_cidata_iso(
    inst: &Instance,
    host_user: &HostUser,
    ssh_public_key: &str,
) -> eyre::Result<()> {
    let user_data = render_user_data(host_user, ssh_public_key)?;
    let meta_data = render_meta_data(inst)?;
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
    ];

    write_cidata_iso(&iso_path, "CIDATA", &files)
        .with_context(|| format!("build cidata ISO at {}", iso_path.display()))?;

    Ok(())
}

#[derive(Serialize)]
struct CloudConfig {
    users: Vec<CloudUser>,
    network: CloudNetwork,
}

#[derive(Serialize)]
struct CloudNetwork {
    config: String,
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

fn render_user_data(host_user: &HostUser, ssh_public_key: &str) -> eyre::Result<String> {
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

    let cloud_config = CloudConfig {
        users: vec![user],
        network: CloudNetwork {
            config: "disabled".to_string(),
        },
    };
    let mut yaml = String::from("#cloud-config\n");
    yaml.push_str(
        &serde_yaml_ng::to_string(&cloud_config).context("serialize cloud-init user-data")?,
    );
    Ok(yaml)
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

    #[test]
    fn user_data_contains_expected_user_fields() {
        let host_user = HostUser {
            name: "nickvd".to_string(),
            uid: 504,
            gecos: "Nick Van Dyck".to_string(),
        };

        let user_data = render_user_data(
            &host_user,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestValue nickvd@host",
        )
        .expect("render user-data");

        assert!(user_data.starts_with("#cloud-config\n"));
        assert!(user_data.contains("name: nickvd"));
        assert!(user_data.contains("uid: 504"));
        assert!(user_data.contains("homedir: /home/nickvd"));
        assert!(user_data.contains("ssh_authorized_keys"));
        assert!(user_data.contains("network:"));
        assert!(user_data.contains("config: disabled"));
    }
}
