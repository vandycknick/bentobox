use std::fmt::{Display, Formatter};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use clap::{Args, Subcommand};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tabwriter::TabWriter;

const OPENAI_CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_DEVICE_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const OPENAI_DEVICE_POLL_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_DEVICE_VERIFY_URL: &str = "https://auth.openai.com/codex/device";
const OPENAI_DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const OPENAI_CODEX_PROVIDER: &str = "openai-codex";
const OPENAI_CODEX_KIND: &str = "openai_codex_oauth";
const OPENAI_DEVICE_LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Args, Debug)]
#[command(
    about = "Manage Bento credential files",
    after_help = "Examples:\n  bento credentials login openai-codex --name personal\n  bento credentials list\n  bento credentials show personal\n  bento credentials rm personal --force\n"
)]
pub struct Cmd {
    #[command(subcommand)]
    pub command: CredentialsSubcommand,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "credentials")
    }
}

#[derive(Subcommand, Debug)]
pub enum CredentialsSubcommand {
    #[command(about = "Log in to a credential provider")]
    Login(LoginCmd),
    #[command(about = "List saved credentials", visible_alias = "ls")]
    List(ListCmd),
    #[command(about = "Show a saved credential")]
    Show(ShowCmd),
    #[command(name = "rm", about = "Remove a saved credential")]
    Rm(RmCmd),
}

#[derive(Args, Debug)]
pub struct LoginCmd {
    /// Credential provider to log in to. Currently: openai-codex.
    #[arg(value_name = "PROVIDER", value_parser = parse_provider)]
    pub provider: CredentialProvider,
    /// Credential name to save.
    #[arg(long)]
    pub name: String,
}

#[derive(Args, Debug)]
pub struct ListCmd {
    /// Output credentials as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ShowCmd {
    /// Credential name to show.
    #[arg(value_name = "NAME")]
    pub name: String,
    /// Output credential metadata as JSON.
    #[arg(long)]
    pub json: bool,
    /// Print only the credential path.
    #[arg(long)]
    pub path: bool,
}

#[derive(Args, Debug)]
pub struct RmCmd {
    /// Credential name to remove.
    #[arg(value_name = "NAME")]
    pub name: String,
    /// Remove without prompting.
    #[arg(long)]
    pub force: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialProvider {
    OpenAICodex,
}

impl CredentialProvider {
    fn directory(self) -> &'static str {
        match self {
            CredentialProvider::OpenAICodex => OPENAI_CODEX_PROVIDER,
        }
    }

    fn kind(self) -> &'static str {
        match self {
            CredentialProvider::OpenAICodex => OPENAI_CODEX_KIND,
        }
    }

    fn label(self) -> &'static str {
        match self {
            CredentialProvider::OpenAICodex => OPENAI_CODEX_PROVIDER,
        }
    }
}

impl Cmd {
    pub async fn run(&self) -> eyre::Result<()> {
        let store = CredentialStore::from_env()?;
        match &self.command {
            CredentialsSubcommand::Login(cmd) => login(&store, cmd).await,
            CredentialsSubcommand::List(cmd) => list_credentials(&store, cmd),
            CredentialsSubcommand::Show(cmd) => show_credential(&store, cmd),
            CredentialsSubcommand::Rm(cmd) => remove_credential(&store, cmd),
        }
    }
}

fn parse_provider(input: &str) -> Result<CredentialProvider, String> {
    match input {
        OPENAI_CODEX_PROVIDER | OPENAI_CODEX_KIND => Ok(CredentialProvider::OpenAICodex),
        other => Err(format!(
            "unsupported credential provider '{other}', expected {OPENAI_CODEX_PROVIDER}"
        )),
    }
}

async fn login(store: &CredentialStore, cmd: &LoginCmd) -> eyre::Result<()> {
    store.ensure_provider_dir(cmd.provider)?;
    let path = store.path(cmd.provider, &cmd.name)?;
    if path.exists() {
        eyre::bail!(
            "credential `{}` already exists at {}",
            cmd.name,
            path.display()
        );
    }

    let token = match cmd.provider {
        CredentialProvider::OpenAICodex => login_openai_codex().await?,
    };
    let now = rfc3339_now();
    let credential = CredentialFile {
        version: 1,
        kind: cmd.provider.kind().to_string(),
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: expires_at_from_seconds(token.expires_in),
        account_id: None,
        created_at: now.clone(),
        updated_at: now,
    };
    store.write(cmd.provider, &cmd.name, &credential)?;

    println!("saved {}", path.display());
    println!();
    print_hcl_snippet(&cmd.name, &path);
    Ok(())
}

async fn login_openai_codex() -> eyre::Result<TokenResponse> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("bentoctl/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()?;

    let device = start_openai_device_flow(&client).await?;
    let interval = Duration::from_secs(device.interval_seconds().unwrap_or(5).max(1));
    println!("Open this URL:");
    println!();
    println!("{}", OPENAI_DEVICE_VERIFY_URL);
    println!();
    println!("Enter code:");
    println!();
    println!("{}", device.user_code);
    println!();
    print!("Waiting for login");
    std::io::stdout().flush()?;

    let deadline = tokio::time::Instant::now() + OPENAI_DEVICE_LOGIN_TIMEOUT;
    loop {
        if tokio::time::Instant::now() >= deadline {
            println!();
            eyre::bail!("timed out waiting for OpenAI Codex login");
        }
        tokio::time::sleep(interval).await;
        match poll_openai_device_flow(&client, &device).await? {
            DevicePoll::Pending => {
                print!(".");
                std::io::stdout().flush()?;
            }
            DevicePoll::Authorized { code, verifier } => {
                println!();
                return exchange_openai_code(&client, &code, &verifier).await;
            }
        }
    }
}

async fn start_openai_device_flow(client: &reqwest::Client) -> eyre::Result<DeviceStartResponse> {
    let response = client
        .post(OPENAI_DEVICE_CODE_URL)
        .json(&serde_json::json!({ "client_id": OPENAI_CODEX_CLIENT_ID }))
        .send()
        .await?;
    let device: DeviceStartResponse =
        decode_json_response(response, "start OpenAI Codex device login").await?;
    if device.device_auth_id.is_empty() || device.user_code.is_empty() {
        eyre::bail!("OpenAI Codex device login returned an incomplete response");
    }
    Ok(device)
}

async fn poll_openai_device_flow(
    client: &reqwest::Client,
    device: &DeviceStartResponse,
) -> eyre::Result<DevicePoll> {
    let response = client
        .post(OPENAI_DEVICE_POLL_URL)
        .json(&serde_json::json!({
            "device_auth_id": &device.device_auth_id,
            "user_code": &device.user_code,
        }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if is_pending_device_poll_response(status, &body) {
        return Ok(DevicePoll::Pending);
    }
    if !status.is_success() {
        eyre::bail!(
            "poll OpenAI Codex device login returned {}: {}",
            status,
            sanitize_response_body(&body)
        );
    }
    let parsed: DevicePollResponse = serde_json::from_str(&body)?;
    if parsed.authorization_code.is_empty() || parsed.code_verifier.is_empty() {
        return Ok(DevicePoll::Pending);
    }
    Ok(DevicePoll::Authorized {
        code: parsed.authorization_code,
        verifier: parsed.code_verifier,
    })
}

async fn exchange_openai_code(
    client: &reqwest::Client,
    code: &str,
    verifier: &str,
) -> eyre::Result<TokenResponse> {
    let response = client
        .post(OPENAI_TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("client_id", OPENAI_CODEX_CLIENT_ID),
            ("redirect_uri", OPENAI_DEVICE_REDIRECT_URI),
        ])
        .send()
        .await?;
    let token: TokenResponse =
        decode_json_response(response, "exchange OpenAI Codex login code").await?;
    if token.access_token.is_empty() {
        eyre::bail!("OpenAI Codex token response did not include an access token");
    }
    if token.refresh_token.is_empty() {
        eyre::bail!("OpenAI Codex token response did not include a refresh token");
    }
    Ok(token)
}

async fn decode_json_response<T>(response: reqwest::Response, context: &str) -> eyre::Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        eyre::bail!(
            "{context} returned {}: {}",
            status,
            sanitize_response_body(&body)
        );
    }
    Ok(serde_json::from_str(&body)?)
}

fn list_credentials(store: &CredentialStore, cmd: &ListCmd) -> eyre::Result<()> {
    let credentials = store.list()?;
    if cmd.json {
        println!("{}", serde_json::to_string_pretty(&credentials)?);
        return Ok(());
    }

    let mut out = TabWriter::new(std::io::stdout()).padding(2);
    writeln!(&mut out, "PROVIDER\tNAME\tEXPIRES_AT\tPATH")?;
    for credential in credentials {
        writeln!(
            &mut out,
            "{}\t{}\t{}\t{}",
            credential.provider,
            credential.name,
            credential.expires_at,
            credential.path.display()
        )?;
    }
    out.flush()?;
    Ok(())
}

fn show_credential(store: &CredentialStore, cmd: &ShowCmd) -> eyre::Result<()> {
    let path = store.path(CredentialProvider::OpenAICodex, &cmd.name)?;
    if cmd.path {
        println!("{}", path.display());
        return Ok(());
    }
    let credential = store.read(CredentialProvider::OpenAICodex, &cmd.name)?;
    let redacted = RedactedCredential::from_file(&credential, &path);
    if cmd.json {
        println!("{}", serde_json::to_string_pretty(&redacted)?);
    } else {
        println!("provider: {}", OPENAI_CODEX_PROVIDER);
        println!("name: {}", cmd.name);
        println!("path: {}", path.display());
        println!("kind: {}", redacted.kind);
        println!("expires_at: {}", redacted.expires_at);
        println!("account_id: {}", redacted.account_id.unwrap_or_default());
        println!("access_token: <redacted>");
        println!("refresh_token: <redacted>");
    }
    Ok(())
}

fn remove_credential(store: &CredentialStore, cmd: &RmCmd) -> eyre::Result<()> {
    if !cmd.force {
        eyre::bail!(
            "refusing to remove credential `{}` without --force",
            cmd.name
        );
    }
    let path = store.path(CredentialProvider::OpenAICodex, &cmd.name)?;
    if !path.exists() {
        eyre::bail!("credential `{}` not found", cmd.name);
    }
    std::fs::remove_file(&path)?;
    println!("removed {}", path.display());
    Ok(())
}

fn print_hcl_snippet(name: &str, path: &Path) {
    println!("Add this credential to an HTTPS endpoint in your policy:");
    println!();
    println!("credential \"{}\" \"{}\" {{", OPENAI_CODEX_KIND, name);
    println!("  endpoint = https.openai-codex");
    println!("  token_file = \"{}\"", path.display());
    println!("}}");
}

#[derive(Debug, Deserialize)]
struct DeviceStartResponse {
    device_auth_id: String,
    user_code: String,
    #[serde(default)]
    interval: Value,
}

impl DeviceStartResponse {
    fn interval_seconds(&self) -> Option<u64> {
        match &self.interval {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.parse::<u64>().ok(),
            _ => None,
        }
    }
}

enum DevicePoll {
    Pending,
    Authorized { code: String, verifier: String },
}

#[derive(Debug, Deserialize)]
struct DevicePollResponse {
    #[serde(default)]
    authorization_code: String,
    #[serde(default)]
    code_verifier: String,
}

#[derive(Debug, Deserialize)]
struct DevicePollErrorResponse {
    error: Option<DevicePollError>,
}

#[derive(Debug, Deserialize)]
struct DevicePollError {
    #[serde(default)]
    code: String,
}

fn is_pending_device_poll_response(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::ACCEPTED || status == StatusCode::NO_CONTENT {
        return true;
    }
    let Ok(response) = serde_json::from_str::<DevicePollErrorResponse>(body) else {
        return false;
    };
    let Some(error) = response.error else {
        return false;
    };
    matches!(
        error.code.as_str(),
        "deviceauth_authorization_pending" | "authorization_pending"
    )
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialFile {
    version: u32,
    kind: String,
    access_token: String,
    refresh_token: String,
    expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct RedactedCredential {
    provider: &'static str,
    path: PathBuf,
    version: u32,
    kind: String,
    expires_at: String,
    account_id: Option<String>,
    created_at: String,
    updated_at: String,
    access_token: &'static str,
    refresh_token: &'static str,
}

impl RedactedCredential {
    fn from_file(credential: &CredentialFile, path: &Path) -> Self {
        Self {
            provider: OPENAI_CODEX_PROVIDER,
            path: path.to_path_buf(),
            version: credential.version,
            kind: credential.kind.clone(),
            expires_at: credential.expires_at.clone(),
            account_id: credential.account_id.clone(),
            created_at: credential.created_at.clone(),
            updated_at: credential.updated_at.clone(),
            access_token: "<redacted>",
            refresh_token: "<redacted>",
        }
    }
}

#[derive(Debug, Serialize)]
struct CredentialSummary {
    provider: &'static str,
    name: String,
    kind: String,
    expires_at: String,
    path: PathBuf,
}

struct CredentialStore {
    root: PathBuf,
}

impl CredentialStore {
    fn from_env() -> eyre::Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| {
                eyre::eyre!("could not resolve ~/.config/bento/credentials from HOME")
            })?;
        Ok(Self {
            root: home.join(".config/bento/credentials"),
        })
    }

    #[cfg(test)]
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn provider_dir(&self, provider: CredentialProvider) -> PathBuf {
        self.root.join(provider.directory())
    }

    fn ensure_provider_dir(&self, provider: CredentialProvider) -> eyre::Result<()> {
        let dir = self.provider_dir(provider);
        std::fs::create_dir_all(&dir)?;
        set_secure_dir_permissions(&self.root)?;
        set_secure_dir_permissions(&dir)?;
        Ok(())
    }

    fn path(&self, provider: CredentialProvider, name: &str) -> eyre::Result<PathBuf> {
        validate_credential_name(name)?;
        Ok(self.provider_dir(provider).join(format!("{name}.json")))
    }

    fn write(
        &self,
        provider: CredentialProvider,
        name: &str,
        credential: &CredentialFile,
    ) -> eyre::Result<()> {
        self.ensure_provider_dir(provider)?;
        let path = self.path(provider, name)?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| eyre::eyre!("invalid credential path {}", path.display()))?;
        let tmp_path = path.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()));
        let mut body = serde_json::to_vec_pretty(credential)?;
        body.push(b'\n');
        write_secure_file(&tmp_path, &body)?;
        set_secure_file_permissions(&tmp_path)?;
        std::fs::rename(&tmp_path, &path)?;
        set_secure_file_permissions(&path)?;
        Ok(())
    }

    fn read(&self, provider: CredentialProvider, name: &str) -> eyre::Result<CredentialFile> {
        let path = self.path(provider, name)?;
        let raw = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    fn list(&self) -> eyre::Result<Vec<CredentialSummary>> {
        let mut credentials = Vec::new();
        self.list_provider(CredentialProvider::OpenAICodex, &mut credentials)?;
        credentials.sort_by(|a, b| a.provider.cmp(b.provider).then(a.name.cmp(&b.name)));
        Ok(credentials)
    }

    fn list_provider(
        &self,
        provider: CredentialProvider,
        credentials: &mut Vec<CredentialSummary>,
    ) -> eyre::Result<()> {
        let dir = self.provider_dir(provider);
        if !dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let raw = std::fs::read_to_string(&path)?;
            let credential = serde_json::from_str::<CredentialFile>(&raw)?;
            credentials.push(CredentialSummary {
                provider: provider.label(),
                name: stem.to_string(),
                kind: credential.kind,
                expires_at: credential.expires_at,
                path,
            });
        }
        Ok(())
    }
}

fn validate_credential_name(name: &str) -> eyre::Result<()> {
    if name.is_empty() {
        eyre::bail!("credential name cannot be empty");
    }
    if name == "." || name == ".." || name.starts_with('.') {
        eyre::bail!("credential name `{name}` is not allowed");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        eyre::bail!("credential name `{name}` may only contain ASCII letters, numbers, dots, underscores, and dashes");
    }
    Ok(())
}

fn expires_at_from_seconds(expires_in: i64) -> String {
    let expires_at = if expires_in > 0 {
        Utc::now() + chrono::Duration::seconds(expires_in)
    } else {
        Utc::now()
    };
    expires_at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn rfc3339_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn sanitize_response_body(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return "<empty>".to_string();
    }
    let mut chars = body.chars();
    let prefix = chars.by_ref().take(512).collect::<String>();
    if chars.next().is_some() {
        format!("{}...", prefix)
    } else {
        prefix
    }
}

#[cfg(unix)]
fn write_secure_file(path: &Path, body: &[u8]) -> eyre::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(body)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_secure_file(path: &Path, body: &[u8]) -> eyre::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(body)?;
    Ok(())
}

#[cfg(unix)]
fn set_secure_dir_permissions(path: &Path) -> eyre::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_secure_dir_permissions(_path: &Path) -> eyre::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_secure_file_permissions(path: &Path) -> eyre::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_secure_file_permissions(_path: &Path) -> eyre::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use reqwest::StatusCode;

    use crate::commands::{BentoCtlCmd, Command};

    use super::{
        is_pending_device_poll_response, CredentialFile, CredentialProvider, CredentialStore,
        CredentialsSubcommand, OPENAI_CODEX_KIND,
    };

    #[test]
    fn credentials_login_openai_codex_parses() {
        let cmd = BentoCtlCmd::try_parse_from([
            "bento",
            "credentials",
            "login",
            "openai-codex",
            "--name",
            "personal",
        ])
        .expect("credentials login should parse");

        let credentials = match cmd.cmd {
            Command::Credentials(cmd) => cmd,
            other => panic!("expected credentials command, got {other:?}"),
        };
        let login = match credentials.command {
            CredentialsSubcommand::Login(cmd) => cmd,
            other => panic!("expected login command, got {other:?}"),
        };

        assert_eq!(login.provider, CredentialProvider::OpenAICodex);
        assert_eq!(login.name, "personal");
    }

    #[test]
    fn credential_store_writes_under_provider_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(dir.path().to_path_buf());
        let credential = CredentialFile {
            version: 1,
            kind: OPENAI_CODEX_KIND.to_string(),
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: "2026-06-02T12:00:00Z".to_string(),
            account_id: None,
            created_at: "2026-06-02T11:00:00Z".to_string(),
            updated_at: "2026-06-02T11:00:00Z".to_string(),
        };

        store
            .write(CredentialProvider::OpenAICodex, "personal", &credential)
            .expect("write credential");

        let path = store
            .path(CredentialProvider::OpenAICodex, "personal")
            .expect("credential path");
        assert_eq!(path, dir.path().join("openai-codex").join("personal.json"));
        let loaded = store
            .read(CredentialProvider::OpenAICodex, "personal")
            .expect("read credential");
        assert_eq!(loaded.refresh_token, "refresh");
    }

    #[test]
    fn credential_store_rejects_path_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(dir.path().to_path_buf());

        assert!(store
            .path(CredentialProvider::OpenAICodex, "../bad")
            .is_err());
        assert!(store
            .path(CredentialProvider::OpenAICodex, ".hidden")
            .is_err());
    }

    #[test]
    fn device_poll_treats_openai_pending_error_as_pending() {
        let body = r#"{
  "error": {
    "message": "Device authorization is pending. Please try again.",
    "type": "invalid_request_error",
    "code": "deviceauth_authorization_pending"
  }
}"#;

        assert!(is_pending_device_poll_response(StatusCode::FORBIDDEN, body));
    }

    #[test]
    fn device_poll_treats_standard_pending_error_as_pending() {
        let body = r#"{"error":{"code":"authorization_pending"}}"#;

        assert!(is_pending_device_poll_response(
            StatusCode::BAD_REQUEST,
            body
        ));
    }

    #[test]
    fn device_poll_does_not_hide_other_errors() {
        let body = r#"{"error":{"code":"invalid_grant"}}"#;

        assert!(!is_pending_device_poll_response(
            StatusCode::FORBIDDEN,
            body
        ));
    }
}
