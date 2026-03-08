use std::path::{Path, PathBuf};
use std::process::Command;

use eyre::Context;
use serde::Serialize;

use crate::extensions::ExtensionsConfig;
use crate::global_config::{ensure_guest_agent_binary, GlobalConfig};
use crate::host_user::HostUser;
use crate::instance::{resolve_mount_location, Instance, InstanceFile, NetworkMode};

const GUEST_AGENT_CIDATA_ENTRY: &str = "bento-guestd";
const GUEST_AGENT_INSTALL_SCRIPT_ENTRY: &str = "bento-install-guest-agent.sh";
const GUEST_AGENT_BOOTSTRAP_SCRIPT: &str = "/var/lib/cloud/scripts/per-boot/00-bento.bootstrap.sh";
const GUESTD_CONFIG_PATH: &str = "/etc/bento/guestd.yaml";
const GUEST_BOOTSTRAP_SCRIPT_CONTENT: &str = include_str!("../scripts/guest-bootstrap.sh");
const GUEST_INSTALL_SCRIPT_CONTENT: &str = include_str!("../scripts/guest-install.sh");

#[derive(Debug, Clone)]
struct CidataEntry {
    name: String,
    contents: Vec<u8>,
}

pub fn build_cidata_iso(
    inst: &Instance,
    host_user: &HostUser,
    ssh_public_key: &str,
) -> eyre::Result<()> {
    let global_config = GlobalConfig::load()?;
    let agent_binary_path = ensure_guest_agent_binary(&global_config)?;
    let guest_agent_binary = std::fs::read(agent_binary_path)
        .with_context(|| format!("read guest agent binary {}", agent_binary_path.display()))?;

    let user_data = render_user_data(inst, host_user, ssh_public_key)?;
    let meta_data = render_meta_data(inst)?;
    let network_config = render_network_config_for_instance(inst)?;
    let iso_path = inst.file(InstanceFile::CidataIso);
    let mut files = vec![
        CidataEntry {
            name: "user-data".to_string(),
            contents: user_data.into_bytes(),
        },
        CidataEntry {
            name: "meta-data".to_string(),
            contents: meta_data.into_bytes(),
        },
        CidataEntry {
            name: GUEST_AGENT_CIDATA_ENTRY.to_string(),
            contents: guest_agent_binary,
        },
        CidataEntry {
            name: GUEST_AGENT_INSTALL_SCRIPT_ENTRY.to_string(),
            contents: GUEST_INSTALL_SCRIPT_CONTENT.as_bytes().to_vec(),
        },
    ];

    if let Some(network_config) = network_config {
        files.push(CidataEntry {
            name: "network-config".to_string(),
            contents: network_config.into_bytes(),
        });
    }

    write_cidata_iso_hdiutil(&iso_path, "CIDATA", &files)
        .with_context(|| format!("build cidata ISO at {}", iso_path.display()))?;

    Ok(())
}

fn write_cidata_iso_hdiutil(
    output_path: &Path,
    volume_label: &str,
    entries: &[CidataEntry],
) -> eyre::Result<()> {
    let staging_root = make_temp_dir("bento-cidata")?;
    for entry in entries {
        let file_path = staging_root.join(&entry.name);
        std::fs::write(&file_path, &entry.contents)
            .with_context(|| format!("write cidata entry {}", file_path.display()))?;
    }

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
    }

    let output_prefix = make_temp_path("bento-cidata-output");
    let status = Command::new("hdiutil")
        .arg("makehybrid")
        .arg("-iso")
        .arg("-joliet")
        .arg("-default-volume-name")
        .arg(volume_label)
        .arg("-o")
        .arg(&output_prefix)
        .arg(&staging_root)
        .status()
        .context("run hdiutil makehybrid")?;

    if !status.success() {
        return Err(eyre::eyre!(
            "hdiutil makehybrid failed with status {}",
            status
        ));
    }

    let generated = resolve_hdiutil_output_path(&output_prefix)?;
    if output_path.exists() {
        std::fs::remove_file(output_path)
            .with_context(|| format!("remove existing output {}", output_path.display()))?;
    }
    std::fs::rename(&generated, output_path).with_context(|| {
        format!(
            "move generated iso from {} to {}",
            generated.display(),
            output_path.display()
        )
    })?;

    let _ = std::fs::remove_dir_all(staging_root);
    Ok(())
}

fn resolve_hdiutil_output_path(prefix: &Path) -> eyre::Result<PathBuf> {
    let candidates = [
        prefix.to_path_buf(),
        PathBuf::from(format!("{}.iso", prefix.display())),
        PathBuf::from(format!("{}.cdr", prefix.display())),
        PathBuf::from(format!("{}.iso.cdr", prefix.display())),
    ];

    candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| eyre::eyre!("hdiutil did not produce an output image"))
}

fn make_temp_dir(prefix: &str) -> eyre::Result<PathBuf> {
    let path = make_temp_path(prefix);
    std::fs::create_dir_all(&path)
        .with_context(|| format!("create temporary directory {}", path.display()))?;
    Ok(path)
}

fn make_temp_path(prefix: &str) -> PathBuf {
    let nonce = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(_) => 0,
    };
    std::env::temp_dir().join(format!("{prefix}-{nonce}"))
}

#[derive(Serialize)]
struct CloudConfig {
    users: Vec<CloudUser>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mounts: Vec<[String; 6]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    write_files: Vec<WriteFile>,
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
struct WriteFile {
    path: String,
    owner: String,
    permissions: String,
    content: String,
}

#[derive(Serialize)]
struct GuestdConfigFile<'a> {
    extensions: &'a ExtensionsConfig,
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
    dhcp4: bool,
    dhcp6: bool,
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

    let guestd_config = serde_yaml_ng::to_string(&GuestdConfigFile {
        extensions: &inst.config.extensions,
    })
    .context("serialize guestd config")?;

    let write_files = vec![
        WriteFile {
            path: GUEST_AGENT_BOOTSTRAP_SCRIPT.to_string(),
            owner: "root:root".to_string(),
            permissions: "0755".to_string(),
            content: GUEST_BOOTSTRAP_SCRIPT_CONTENT.to_string(),
        },
        WriteFile {
            path: GUESTD_CONFIG_PATH.to_string(),
            owner: "root:root".to_string(),
            permissions: "0644".to_string(),
            content: guestd_config,
        },
    ];

    let cloud_config = CloudConfig {
        users: vec![user],
        mounts,
        write_files,
    };
    let mut bento_yaml = String::from("#cloud-config\n");
    bento_yaml.push_str(
        &serde_yaml_ng::to_string(&cloud_config).context("serialize cloud-init user-data")?,
    );

    if let Some(userdata_path) = inst.config.userdata_path.as_ref() {
        let user_data = std::fs::read_to_string(userdata_path)
            .with_context(|| format!("read userdata {}", userdata_path.display()))?;
        return Ok(render_multipart_user_data(&bento_yaml, &user_data));
    }

    Ok(bento_yaml)
}

fn render_multipart_user_data(bento_user_data: &str, user_data: &str) -> String {
    let boundary = "===============bento-userdata==";
    format!(
        "MIME-Version: 1.0\nContent-Type: multipart/mixed; boundary=\"{boundary}\"\n\n--{boundary}\nContent-Type: text/cloud-config; charset=\"us-ascii\"\n\n{bento_user_data}\n--{boundary}\nContent-Type: {user_content_type}; charset=\"us-ascii\"\n\n{user_data}\n--{boundary}--\n",
        boundary = boundary,
        bento_user_data = bento_user_data.trim_end(),
        user_content_type = detect_userdata_content_type(user_data),
        user_data = user_data.trim_end(),
    )
}

fn detect_userdata_content_type(user_data: &str) -> &'static str {
    let trimmed = user_data.trim_start();
    if trimmed.starts_with("#cloud-config") {
        "text/cloud-config"
    } else if trimmed.starts_with("#!") {
        "text/x-shellscript"
    } else {
        "text/plain"
    }
}

fn render_network_config() -> eyre::Result<String> {
    let mut ethernets = std::collections::BTreeMap::new();
    ethernets.insert(
        "enp0s1".to_string(),
        EthernetConfigV2 {
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

fn render_network_config_for_instance(inst: &Instance) -> eyre::Result<Option<String>> {
    match inst.resolved_network_mode() {
        NetworkMode::VzNat => render_network_config().map(Some),
        NetworkMode::None => Ok(None),
        NetworkMode::Bridged => Err(eyre::eyre!("network mode 'bridged' is not implemented yet")),
        NetworkMode::Cni => Err(eyre::eyre!("network mode 'cni' is not implemented yet")),
    }
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
    use crate::instance::{InstanceConfig, MountConfig, NetworkConfig};
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

    #[test]
    fn user_data_contains_guest_agent_install_steps() {
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

        assert!(user_data.contains(GUEST_AGENT_BOOTSTRAP_SCRIPT));
        assert!(user_data.contains(GUEST_AGENT_INSTALL_SCRIPT_ENTRY));
        assert!(user_data.contains("/run/bento-cidata"));
        assert!(user_data.contains("/dev/disk/by-label/CIDATA"));
    }

    #[test]
    fn guest_agent_payload_contains_systemd_install_steps() {
        let payload = GUEST_INSTALL_SCRIPT_CONTENT;

        assert!(payload.contains("/usr/local/bin/bento-guestd"));
        assert!(payload.contains("/etc/systemd/system/bento-guestd.service"));
        assert!(payload.contains("systemctl daemon-reload"));
        assert!(payload.contains("systemctl enable"));
    }

    #[test]
    fn network_config_is_generated_for_vznat() {
        let mut config = InstanceConfig::new();
        config.network = Some(NetworkConfig {
            mode: NetworkMode::VzNat,
        });
        let inst = Instance::new("vm1".to_string(), PathBuf::from("/tmp/vm1"), config);

        let network_config =
            render_network_config_for_instance(&inst).expect("network config should render");
        let rendered = network_config.expect("vznat should emit network-config");
        assert!(rendered.contains("version: 2"));
        assert!(rendered.contains("dhcp4: true"));
    }

    #[test]
    fn network_config_is_omitted_for_none_mode() {
        let mut config = InstanceConfig::new();
        config.network = Some(NetworkConfig {
            mode: NetworkMode::None,
        });
        let inst = Instance::new("vm1".to_string(), PathBuf::from("/tmp/vm1"), config);

        let network_config =
            render_network_config_for_instance(&inst).expect("none mode should be valid");
        assert!(network_config.is_none());
    }
}
