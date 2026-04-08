use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{self, Seek, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use bento_core::services::GuestRuntimeConfig;
use bento_core::{resolve_mount_location, InstanceFile};
use bento_libvm::global_config::{ensure_guest_agent_binary, GlobalConfig};
use bento_libvm::host_user::{self, HostUser};
use bento_libvm::ssh_keys;
use eyre::Context;
use fatfs::{format_volume, FileSystem, FormatVolumeOptions, FsOptions};
use serde::Serialize;

use crate::monitor_config::VmContext;

const GUEST_AGENT_CIDATA_ENTRY: &str = "bento-guestd";
const GUEST_AGENT_INSTALL_SCRIPT_ENTRY: &str = "bento-install-guest-agent.sh";
const GUEST_AGENT_CONFIG_ENTRY: &str = "bento-guestd.yaml";
const GUEST_AGENT_CONFIG_ENV_ENTRY: &str = "config.env";
const GUEST_AGENT_BOOTSTRAP_SCRIPT: &str = "/var/lib/cloud/scripts/per-boot/00-bento.bootstrap.sh";
const GUEST_BOOTSTRAP_SCRIPT_CONTENT: &str = include_str!("../scripts/guest-bootstrap.sh");
const GUEST_INSTALL_SCRIPT_CONTENT: &str = include_str!("../scripts/guest-install.sh");
const TASK_REGISTER_GUESTD_CONTENT: &str = include_str!("../scripts/tasks/10-register-guestd.sh");
const TASK_SETUP_ROSETTA_CONTENT: &str = include_str!("../scripts/tasks/20-setup-rosetta.sh");
const CIDATA_VOLUME_LABEL: &str = "CIDATA";
const CIDATA_MIN_SIZE_BYTES: u64 = 16 * 1024 * 1024;
const CIDATA_SIZE_OVERHEAD_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
struct CidataEntry {
    name: String,
    contents: Vec<u8>,
}

pub(crate) fn rebuild_bootstrap(
    config: &VmContext,
    guest_runtime: &GuestRuntimeConfig,
) -> eyre::Result<()> {
    if !config.requires_bootstrap_for(guest_runtime) {
        let iso_path = config.file(InstanceFile::CidataDisk);
        if iso_path.exists() {
            std::fs::remove_file(&iso_path)
                .with_context(|| format!("remove stale cidata {}", iso_path.display()))?;
        }
        return Ok(());
    }

    let host_user = host_user::current_host_user().context("resolve current host user")?;
    let user_keys = ssh_keys::ensure_user_ssh_keys().context("ensure user SSH keys")?;

    build_cidata_disk(
        config,
        &host_user,
        &user_keys.public_key_openssh,
        guest_runtime,
    )
}

fn build_cidata_disk(
    config: &VmContext,
    host_user: &HostUser,
    ssh_public_key: &str,
    guest_runtime: &GuestRuntimeConfig,
) -> eyre::Result<()> {
    let global_config = GlobalConfig::load()?;
    let agent_binary_path = ensure_guest_agent_binary(&global_config)?;
    let guest_agent_binary = std::fs::read(agent_binary_path)
        .with_context(|| format!("read guest agent binary {}", agent_binary_path.display()))?;

    let desired_state = desired_guestd_state(guest_runtime)?;
    let user_data = render_user_data(config, host_user, ssh_public_key, &desired_state)?;
    let meta_data = render_meta_data(config)?;
    let network_config = render_network_config_for_instance(config)?;
    let guestd_config = render_guestd_config(&desired_state)?;
    let config_env = render_config_env(config, &desired_state)?;
    let iso_path = config.file(InstanceFile::CidataDisk);

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
        CidataEntry {
            name: GUEST_AGENT_CONFIG_ENTRY.to_string(),
            contents: guestd_config.into_bytes(),
        },
        CidataEntry {
            name: GUEST_AGENT_CONFIG_ENV_ENTRY.to_string(),
            contents: config_env.into_bytes(),
        },
        CidataEntry {
            name: "tasks/10-register-guestd.sh".to_string(),
            contents: TASK_REGISTER_GUESTD_CONTENT.as_bytes().to_vec(),
        },
    ];

    if config.rosetta_enabled() {
        files.push(CidataEntry {
            name: "tasks/20-setup-rosetta.sh".to_string(),
            contents: TASK_SETUP_ROSETTA_CONTENT.as_bytes().to_vec(),
        });
    }

    if let Some(network_config) = network_config {
        files.push(CidataEntry {
            name: "network-config".to_string(),
            contents: network_config.into_bytes(),
        });
    }

    write_cidata_fat_image(&iso_path, &files)
        .with_context(|| format!("build cidata disk at {}", iso_path.display()))?;

    Ok(())
}

fn write_cidata_fat_image(output_path: &Path, entries: &[CidataEntry]) -> eyre::Result<()> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
    }
    if output_path.exists() {
        std::fs::remove_file(output_path)
            .with_context(|| format!("remove existing output {}", output_path.display()))?;
    }

    // Use a VFAT volume with the NoCloud `CIDATA` label so the same bootstrap media works on
    // both VZ and Firecracker without depending on host-specific ISO tooling.
    let mut image = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(output_path)
        .with_context(|| format!("create cidata image {}", output_path.display()))?;
    image
        .set_len(cidata_image_size(entries))
        .with_context(|| format!("size cidata image {}", output_path.display()))?;

    let mut label = [b' '; 11];
    label[..CIDATA_VOLUME_LABEL.len()].copy_from_slice(CIDATA_VOLUME_LABEL.as_bytes());
    format_volume(&mut image, FormatVolumeOptions::new().volume_label(label))
        .context("format cidata FAT volume")?;
    image.rewind().context("rewind cidata image after format")?;

    let fs = FileSystem::new(image, FsOptions::new()).context("mount cidata FAT volume")?;
    let root = fs.root_dir();
    for entry in entries {
        write_cidata_entry(&root, entry)
            .with_context(|| format!("write cidata entry {}", entry.name))?;
    }

    drop(root);
    fs.unmount().context("flush cidata FAT volume")?;
    Ok(())
}

fn cidata_image_size(entries: &[CidataEntry]) -> u64 {
    // Keep the image comfortably larger than the payload so FAT metadata and directory growth do
    // not force fragile exact sizing logic.
    let payload_bytes = entries
        .iter()
        .map(|entry| entry.contents.len() as u64 + entry.name.len() as u64)
        .sum::<u64>();
    (payload_bytes + CIDATA_SIZE_OVERHEAD_BYTES).max(CIDATA_MIN_SIZE_BYTES)
}

fn write_cidata_entry(
    root: &fatfs::Dir<'_, std::fs::File>,
    entry: &CidataEntry,
) -> eyre::Result<()> {
    let mut parts = entry.name.split('/').peekable();
    let mut current = root.clone();

    while let Some(part) = parts.next() {
        if parts.peek().is_some() {
            current = match current.open_dir(part) {
                Ok(dir) => dir,
                Err(err) if err.kind() == io::ErrorKind::NotFound => current
                    .create_dir(part)
                    .with_context(|| format!("create cidata directory {part}"))?,
                Err(err) => {
                    return Err(err).with_context(|| format!("open cidata directory {part}"))
                }
            };
        } else {
            let mut file = current
                .create_file(part)
                .with_context(|| format!("create cidata file {part}"))?;
            file.truncate().context("truncate cidata file")?;
            file.write_all(&entry.contents)
                .with_context(|| format!("write cidata file {part}"))?;
            file.flush().context("flush cidata file")?;
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct CloudConfig {
    users: Vec<CloudUser>,
    growpart: GrowpartConfig,
    resize_rootfs: bool,
    timezone: String,
    locale: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mounts: Vec<[String; 6]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    write_files: Vec<WriteFile>,
}

#[derive(Serialize)]
struct GrowpartConfig {
    mode: String,
    devices: Vec<String>,
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

fn desired_guestd_state(guest_runtime: &GuestRuntimeConfig) -> eyre::Result<GuestRuntimeConfig> {
    Ok(guest_runtime.clone())
}

fn render_user_data(
    config: &VmContext,
    host_user: &HostUser,
    ssh_public_key: &str,
    _guest_runtime: &GuestRuntimeConfig,
) -> eyre::Result<String> {
    let timezone = resolve_host_timezone();
    let locale = resolve_host_locale();

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

    let write_files = vec![WriteFile {
        path: GUEST_AGENT_BOOTSTRAP_SCRIPT.to_string(),
        owner: "root:root".to_string(),
        permissions: "0755".to_string(),
        content: GUEST_BOOTSTRAP_SCRIPT_CONTENT.to_string(),
    }];

    let cloud_config = CloudConfig {
        users: vec![user],
        growpart: GrowpartConfig {
            mode: "auto".to_string(),
            devices: vec!["/".to_string()],
        },
        resize_rootfs: true,
        timezone,
        locale,
        mounts: cloud_mount_entries(config),
        write_files,
    };
    let mut bento_yaml = String::from("#cloud-config\n");
    bento_yaml.push_str(
        &serde_yaml_ng::to_string(&cloud_config).context("serialize cloud-init user-data")?,
    );

    if let Some(userdata_path) = config.userdata_path() {
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

fn render_network_config_for_instance(config: &VmContext) -> eyre::Result<Option<String>> {
    match config.resolved_network_mode() {
        bento_vmm::NetworkMode::VzNat => render_network_config().map(Some),
        bento_vmm::NetworkMode::None => Ok(None),
        bento_vmm::NetworkMode::Bridged => {
            Err(eyre::eyre!("network mode 'bridged' is not implemented yet"))
        }
        bento_vmm::NetworkMode::Cni => {
            Err(eyre::eyre!("network mode 'cni' is not implemented yet"))
        }
    }
}

fn render_meta_data(config: &VmContext) -> eyre::Result<String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let mut hasher = DefaultHasher::new();
    config.name.hash(&mut hasher);
    nonce.hash(&mut hasher);
    let hash = hasher.finish();

    let metadata = MetaData {
        instance_id: format!("bento-{:08x}", (hash >> 32) as u32),
        local_hostname: config.name.clone(),
    };
    serde_yaml_ng::to_string(&metadata).context("serialize cloud-init meta-data")
}

fn cloud_mount_entries(config: &VmContext) -> Vec<[String; 6]> {
    config
        .mounts()
        .iter()
        .map(|mount| {
            let path = resolve_mount_location(&mount.source)
                .unwrap_or_else(|_| mount.source.clone())
                .to_string_lossy()
                .to_string();
            [
                mount.tag.clone(),
                path,
                "virtiofs".to_string(),
                if mount.writable {
                    "rw,nofail".to_string()
                } else {
                    "ro,nofail".to_string()
                },
                "0".to_string(),
                "0".to_string(),
            ]
        })
        .collect()
}

fn render_guestd_config(guest_runtime: &GuestRuntimeConfig) -> eyre::Result<String> {
    serde_yaml_ng::to_string(guest_runtime).context("serialize guestd config")
}

fn render_config_env(
    config: &VmContext,
    _guest_runtime: &GuestRuntimeConfig,
) -> eyre::Result<String> {
    let mut env = String::new();

    env.push_str(&format!(
        "BENTO_INSTANCE_NAME={}\n",
        shell_quote(&config.name)
    ));
    env.push_str("BENTO_GUESTD_BINARY_PATH=/usr/local/bin/bento-guestd\n");
    env.push_str("BENTO_GUESTD_CONFIG_PATH=/etc/bento/guestd.yaml\n");
    env.push_str(&format!(
        "BENTO_ROSETTA={}\n",
        if config.rosetta_enabled() {
            "true"
        } else {
            "false"
        }
    ));

    Ok(env)
}

fn shell_quote(value: &str) -> String {
    let escaped = value.replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn resolve_host_timezone() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        let trimmed = tz.trim();
        if !trimmed.is_empty() {
            return trimmed.trim_start_matches(':').to_string();
        }
    }

    if let Ok(localtime_target) = std::fs::read_link("/etc/localtime") {
        let rendered = localtime_target.to_string_lossy();
        if let Some((_, timezone)) = rendered.split_once("zoneinfo/") {
            let timezone = timezone.trim_matches('/');
            if !timezone.is_empty() {
                return timezone.to_string();
            }
        }
    }

    if let Ok(contents) = std::fs::read_to_string("/etc/timezone") {
        if let Some(first_line) = contents.lines().next() {
            let timezone = first_line.trim();
            if !timezone.is_empty() {
                return timezone.to_string();
            }
        }
    }

    tracing::warn!("unable to determine host timezone, defaulting guest timezone to UTC");
    "UTC".to_string()
}

fn resolve_host_locale() -> String {
    for var in ["LC_ALL", "LANG"] {
        if let Ok(value) = std::env::var(var) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    tracing::warn!("unable to determine host locale, defaulting guest locale to en_US.UTF-8");
    "en_US.UTF-8".to_string()
}
