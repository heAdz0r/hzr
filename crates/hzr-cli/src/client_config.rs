use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use serde::Serialize;
use serde_json::{Map, Value, json};
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::adoption::{atomic_write, backup_path, commit, read_optional, sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Client {
    Codex,
    ClaudeDesktop,
    /// Claude Code's own state file. Audited for ownership, never written by HZR: it holds
    /// far more than MCP registrations, so rewriting it would put HZR in charge of the
    /// user's session state.
    ClaudeCode,
}

impl Client {
    /// How the user removes a direct `icm` server for this client. It differs by client
    /// because HZR rewrites two of these configurations and must never rewrite the third —
    /// pointing every client at `hzr mcp config` told Claude Code users to run a command
    /// that would not touch their file.
    pub fn direct_icm_remediation(self) -> &'static str {
        match self {
            Self::Codex | Self::ClaudeDesktop => {
                "HZR owns this registration: run `hzr install --dry-run`, then \
                 `hzr install --force` to replace it; `hzr mcp config --client <client> \
                 --workspace <dir>` only prints a snippet to paste by hand"
            }
            Self::ClaudeCode => {
                "HZR never writes this file: remove the server with `claude mcp remove icm`, \
                 then add HZR with `claude mcp add hzr -- hzr mcp serve --workspace <dir>`"
            }
        }
    }
}

impl Client {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeDesktop => "claude-desktop",
            Self::ClaudeCode => "claude-code",
        }
    }

    /// Whether `hzr install` may rewrite this client's configuration.
    fn is_writable(self) -> bool {
        !matches!(self, Self::ClaudeCode)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ClientConfigReport {
    pub client: Client,
    pub path: PathBuf,
    pub changed: bool,
    pub direct_icm_removed: usize,
    pub hzr_registered: bool,
    pub backup_path: Option<PathBuf>,
    pub before_sha256: String,
    pub after_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ClientMcpStatus {
    pub client: Client,
    pub path: PathBuf,
    pub config_exists: bool,
    pub registered: bool,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub direct_icm_registrations: usize,
    /// The project pinned with `--workspace`, when the registration pins one. Unpinned means
    /// the memory namespace is decided by whatever directory the client launches from, which
    /// is `/` for the Claude desktop app and a per-session directory for Codex.
    pub pinned_workspace: Option<String>,
    pub lifecycle: &'static str,
    pub started_by_init: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Registration {
    command: String,
    args: Vec<String>,
}

impl Registration {
    /// `mcp serve` must be matched as a prefix, not an exact argument list: the recommended
    /// registration also carries `--workspace <dir>`, and an exact comparison reported those
    /// correctly-configured clients as unregistered.
    fn serves_mcp(&self) -> bool {
        self.args.len() >= 2 && self.args[0] == "mcp" && self.args[1] == "serve"
    }

    fn is_native_hzr(&self) -> bool {
        Path::new(&self.command)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "hzr")
            && self.serves_mcp()
    }

    fn matches(&self, binary: &Path) -> bool {
        self.command == binary.to_string_lossy() && self.serves_mcp()
    }

    /// The value passed to `--workspace`, if any.
    fn pinned_workspace(&self) -> Option<String> {
        self.args
            .iter()
            .position(|argument| argument == "--workspace")
            .and_then(|index| self.args.get(index + 1))
            .cloned()
    }
}

pub const MCP_LIFECYCLE: &str = "client_managed_stdio";

/// Clients whose configuration `hzr install` may write.
pub fn default_paths() -> Result<Vec<(Client, PathBuf)>> {
    Ok(audit_paths()?
        .into_iter()
        .filter(|(client, _)| client.is_writable())
        .collect())
}

/// Every client configuration HZR reads when auditing memory ownership, including the ones
/// it must never write. A registration HZR cannot fix is still a registration that creates a
/// second memory writer, so leaving it unread is what let a direct `icm` server in
/// `~/.claude.json` pass the ownership check for an entire release.
pub fn audit_paths() -> Result<Vec<(Client, PathBuf)>> {
    let base = BaseDirs::new().context("cannot determine the user home directory")?;
    let home = base.home_dir();
    let codex = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
        .join("config.toml");
    let mut paths = vec![(Client::Codex, codex)];
    if let Some(path) = std::env::var_os("CLAUDE_DESKTOP_CONFIG").map(PathBuf::from) {
        paths.push((Client::ClaudeDesktop, path));
    } else if cfg!(target_os = "macos") {
        paths.push((
            Client::ClaudeDesktop,
            home.join("Library/Application Support/Claude/claude_desktop_config.json"),
        ));
    }
    paths.push((
        Client::ClaudeCode,
        std::env::var_os("CLAUDE_CONFIG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude.json")),
    ));
    Ok(paths)
}

pub fn install_all(
    binary: &Path,
    dry_run: bool,
    confirmed: bool,
) -> Result<Vec<ClientConfigReport>> {
    default_paths()?
        .into_iter()
        .map(|(client, path)| install(client, &path, binary, dry_run, confirmed))
        .collect()
}

pub fn uninstall_all(dry_run: bool, confirmed: bool) -> Result<Vec<ClientConfigReport>> {
    default_paths()?
        .into_iter()
        .map(|(client, path)| uninstall(client, &path, dry_run, confirmed))
        .collect()
}

fn uninstall(
    client: Client,
    path: &Path,
    dry_run: bool,
    confirmed: bool,
) -> Result<ClientConfigReport> {
    let before = read_optional(path)?;
    let after = match client {
        Client::Codex => remove_codex_hzr(path, &before)?,
        Client::ClaudeDesktop => remove_json_hzr(path, &before)?,
        Client::ClaudeCode => anyhow::bail!("Claude Code MCP state is audited, never written"),
    };
    let changed = before != after.as_bytes();
    let backup = (changed && !before.is_empty()).then(|| backup_path(path, &before));
    if changed && !dry_run {
        if !confirmed {
            bail!(
                "installation changes {}; inspect `hzr install --dry-run`, then rerun with `--force` to confirm",
                path.display()
            );
        }
        match backup.as_ref() {
            Some(backup) => commit(path, &before, after.as_bytes(), backup, b"")?,
            None => atomic_write(path, after.as_bytes())?,
        }
    }
    Ok(ClientConfigReport {
        client,
        path: path.to_path_buf(),
        changed,
        direct_icm_removed: 0,
        hzr_registered: false,
        backup_path: backup,
        before_sha256: sha256(&before),
        after_sha256: sha256(after.as_bytes()),
    })
}

pub fn status_all() -> Result<Vec<ClientMcpStatus>> {
    audit_paths()?
        .into_iter()
        .map(|(client, path)| status(client, &path))
        .collect()
}

pub fn status(client: Client, path: &Path) -> Result<ClientMcpStatus> {
    let config_exists = path.is_file();
    let bytes = read_optional(path)?;
    let (registration, direct_icm_registrations) = match client {
        Client::Codex => {
            let text = if bytes.is_empty() {
                ""
            } else {
                std::str::from_utf8(&bytes)
                    .with_context(|| format!("{} is not UTF-8", path.display()))?
            };
            let document = text
                .parse::<DocumentMut>()
                .with_context(|| format!("failed to parse {}", path.display()))?;
            (
                codex_hzr_registration(&document),
                codex_direct_icm_count(&document),
            )
        }
        Client::ClaudeDesktop | Client::ClaudeCode => {
            let document = if bytes.is_empty() {
                json!({})
            } else {
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("failed to parse {}", path.display()))?
            };
            (
                json_hzr_registration(&document),
                json_direct_icm_count(&document),
            )
        }
    };
    let registered = registration
        .as_ref()
        .is_some_and(Registration::is_native_hzr);
    let pinned_workspace = registration
        .as_ref()
        .and_then(Registration::pinned_workspace);
    let (command, args) = registration
        .map(|registration| (Some(registration.command), registration.args))
        .unwrap_or_default();

    Ok(ClientMcpStatus {
        client,
        path: path.to_path_buf(),
        config_exists,
        registered,
        command,
        args,
        direct_icm_registrations,
        pinned_workspace,
        lifecycle: MCP_LIFECYCLE,
        started_by_init: false,
    })
}

pub fn direct_icm_registrations() -> Result<Vec<String>> {
    let mut found = Vec::new();
    for (client, path) in audit_paths()? {
        let bytes = read_optional(&path)?;
        if bytes.is_empty() {
            continue;
        }
        let count = match client {
            Client::Codex => {
                let text = std::str::from_utf8(&bytes)
                    .with_context(|| format!("{} is not UTF-8", path.display()))?;
                let document = text
                    .parse::<DocumentMut>()
                    .with_context(|| format!("failed to parse {}", path.display()))?;
                codex_direct_icm_count(&document)
            }
            Client::ClaudeDesktop | Client::ClaudeCode => {
                let document: Value = serde_json::from_slice(&bytes)
                    .with_context(|| format!("failed to parse {}", path.display()))?;
                json_direct_icm_count(&document)
            }
        };
        if count > 0 {
            found.push(format!(
                "{} ({}, {} registration(s) — {})",
                client.as_str(),
                path.display(),
                count,
                client.direct_icm_remediation()
            ));
        }
    }
    Ok(found)
}

pub fn install(
    client: Client,
    path: &Path,
    binary: &Path,
    dry_run: bool,
    confirmed: bool,
) -> Result<ClientConfigReport> {
    let before = read_optional(path)?;
    let (after, direct_icm_removed, hzr_registered) = match client {
        Client::Codex => migrate_codex(path, &before, binary)?,
        Client::ClaudeDesktop => migrate_claude_desktop(path, &before, binary)?,
        // Claude Code's state file is audit-only. Refusing here rather than silently
        // skipping keeps the "HZR owns its own files" rule a checked invariant instead of a
        // convention that a future caller can quietly break.
        Client::ClaudeCode => anyhow::bail!(
            "{} is Claude Code's own state file and is audited, never written by HZR; \
             remove a direct icm server yourself, or register hzr with \
             `claude mcp add`",
            path.display()
        ),
    };
    let changed = before != after.as_bytes();
    let backup = (changed && !before.is_empty()).then(|| backup_path(path, &before));

    if changed && !dry_run {
        if !confirmed {
            bail!(
                "installation changes {}; inspect `hzr install --dry-run`, then rerun with `--force` to confirm",
                path.display()
            );
        }
        match backup.as_ref() {
            Some(backup) => commit(path, &before, after.as_bytes(), backup, b"")?,
            None => atomic_write(path, after.as_bytes())?,
        }
    }

    Ok(ClientConfigReport {
        client,
        path: path.to_path_buf(),
        changed,
        direct_icm_removed,
        hzr_registered,
        backup_path: backup,
        before_sha256: sha256(&before),
        after_sha256: sha256(after.as_bytes()),
    })
}

fn command_launches_icm(command: &str) -> bool {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "icm")
}

fn codex_direct_icm_count(document: &DocumentMut) -> usize {
    document
        .get("mcp_servers")
        .and_then(Item::as_table)
        .map(|servers| {
            servers
                .iter()
                .filter(|(_, item)| {
                    item.as_table()
                        .and_then(|table| table.get("command"))
                        .and_then(Item::as_str)
                        .is_some_and(command_launches_icm)
                })
                .count()
        })
        .unwrap_or(0)
}

fn codex_hzr_registration(document: &DocumentMut) -> Option<Registration> {
    let hzr = document
        .get("mcp_servers")
        .and_then(Item::as_table)
        .and_then(|servers| servers.get("hzr"))
        .and_then(Item::as_table)?;
    Some(Registration {
        command: hzr.get("command")?.as_str()?.to_owned(),
        args: hzr
            .get("args")
            .and_then(Item::as_array)
            .and_then(|args| {
                args.iter()
                    .map(|value| value.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn migrate_codex(path: &Path, before: &[u8], binary: &Path) -> Result<(String, usize, bool)> {
    let text = if before.is_empty() {
        String::new()
    } else {
        std::str::from_utf8(before)
            .with_context(|| format!("{} is not UTF-8; HZR will not rewrite it", path.display()))?
            .to_owned()
    };
    let mut document = text
        .parse::<DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let direct_before = codex_direct_icm_count(&document);
    let hzr_matches = codex_hzr_registration(&document)
        .as_ref()
        .is_some_and(|registration| registration.matches(binary));
    if direct_before == 0 && hzr_matches {
        return Ok((text, 0, true));
    }
    if !document.as_table().contains_key("mcp_servers") {
        document["mcp_servers"] = Item::Table(Table::new());
    }
    let servers = document["mcp_servers"]
        .as_table_mut()
        .with_context(|| format!("mcp_servers in {} is not a table", path.display()))?;
    let direct: Vec<String> = servers
        .iter()
        .filter_map(|(name, item)| {
            item.as_table()
                .and_then(|table| table.get("command"))
                .and_then(Item::as_str)
                .filter(|command| command_launches_icm(command))
                .map(|_| name.to_owned())
        })
        .collect();
    for name in &direct {
        servers.remove(name);
    }

    if !servers.contains_key("hzr") {
        servers.insert("hzr", Item::Table(Table::new()));
    }
    let hzr = servers["hzr"]
        .as_table_mut()
        .with_context(|| format!("mcp_servers.hzr in {} is not a table", path.display()))?;
    hzr["command"] = value(binary.to_string_lossy().as_ref());
    let mut args = Array::new();
    args.push("mcp");
    args.push("serve");
    hzr["args"] = value(args);

    Ok((document.to_string(), direct.len(), true))
}

fn remove_codex_hzr(path: &Path, before: &[u8]) -> Result<String> {
    if before.is_empty() {
        return Ok(String::new());
    }
    let text = std::str::from_utf8(before)
        .with_context(|| format!("{} is not UTF-8; HZR will not rewrite it", path.display()))?;
    let mut document = text
        .parse::<DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let owned =
        codex_hzr_registration(&document).is_some_and(|registration| registration.is_native_hzr());
    if !owned {
        return Ok(text.to_owned());
    }
    if let Some(servers) = document.get_mut("mcp_servers").and_then(Item::as_table_mut) {
        servers.remove("hzr");
    }
    Ok(document.to_string())
}

fn json_servers(document: &Value) -> Option<&Map<String, Value>> {
    document.get("mcpServers").and_then(Value::as_object)
}

fn count_direct_icm(servers: Option<&Map<String, Value>>) -> usize {
    servers
        .map(|servers| {
            servers
                .values()
                .filter(|server| {
                    server
                        .get("command")
                        .and_then(Value::as_str)
                        .is_some_and(command_launches_icm)
                })
                .count()
        })
        .unwrap_or(0)
}

/// Count direct `icm` servers in a JSON client configuration.
///
/// Claude Code additionally keeps per-project registrations under
/// `projects.<path>.mcpServers`, and a second writer registered there is exactly as harmful
/// as one at the top level, so both scopes are counted.
fn json_direct_icm_count(document: &Value) -> usize {
    let top_level = count_direct_icm(json_servers(document));
    let per_project: usize = document
        .get("projects")
        .and_then(Value::as_object)
        .map(|projects| {
            projects
                .values()
                .map(|project| count_direct_icm(json_servers(project)))
                .sum()
        })
        .unwrap_or(0);
    top_level + per_project
}

fn json_hzr_registration(document: &Value) -> Option<Registration> {
    let hzr = json_servers(document)?.get("hzr")?;
    Some(Registration {
        command: hzr.get("command")?.as_str()?.to_owned(),
        args: hzr
            .get("args")
            .and_then(Value::as_array)
            .and_then(|args| {
                args.iter()
                    .map(|value| value.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn migrate_claude_desktop(
    path: &Path,
    before: &[u8],
    binary: &Path,
) -> Result<(String, usize, bool)> {
    let mut document = if before.is_empty() {
        json!({})
    } else {
        serde_json::from_slice::<Value>(before)
            .with_context(|| format!("failed to parse {}", path.display()))?
    };
    let direct_before = json_direct_icm_count(&document);
    let hzr_matches = json_hzr_registration(&document)
        .as_ref()
        .is_some_and(|registration| registration.matches(binary));
    if direct_before == 0 && hzr_matches {
        let text = std::str::from_utf8(before)
            .with_context(|| format!("{} is not UTF-8", path.display()))?
            .to_owned();
        return Ok((text, 0, true));
    }
    let root = document
        .as_object_mut()
        .with_context(|| format!("{} must contain a JSON object", path.display()))?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .with_context(|| format!("mcpServers in {} is not an object", path.display()))?;
    let direct: Vec<String> = servers
        .iter()
        .filter_map(|(name, server)| {
            server
                .get("command")
                .and_then(Value::as_str)
                .filter(|command| command_launches_icm(command))
                .map(|_| name.clone())
        })
        .collect();
    for name in &direct {
        servers.remove(name);
    }
    let hzr = servers.entry("hzr").or_insert_with(|| json!({}));
    let hzr = hzr
        .as_object_mut()
        .with_context(|| format!("mcpServers.hzr in {} is not an object", path.display()))?;
    hzr.insert(
        "command".to_owned(),
        Value::String(binary.to_string_lossy().into_owned()),
    );
    hzr.insert("args".to_owned(), json!(["mcp", "serve"]));

    let mut rendered = serde_json::to_string_pretty(&document)?;
    rendered.push('\n');
    Ok((rendered, direct.len(), true))
}

fn remove_json_hzr(path: &Path, before: &[u8]) -> Result<String> {
    if before.is_empty() {
        return Ok(String::new());
    }
    let mut document = serde_json::from_slice::<Value>(before)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let owned =
        json_hzr_registration(&document).is_some_and(|registration| registration.is_native_hzr());
    if !owned {
        return String::from_utf8(before.to_vec())
            .with_context(|| format!("{} is not UTF-8", path.display()));
    }
    if let Some(servers) = document
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
    {
        servers.remove("hzr");
    }
    let mut rendered = serde_json::to_string_pretty(&document)?;
    rendered.push('\n');
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::Value;
    use tempfile::tempdir;
    use toml_edit::DocumentMut;

    use super::{Client, MCP_LIFECYCLE, audit_paths, default_paths, install, status, uninstall};

    fn binary() -> &'static Path {
        Path::new("/opt/hzr/current/bin/hzr")
    }

    /// A registration that pins its workspace is the recommended form, so recognition must
    /// not depend on the argument list being exactly `["mcp", "serve"]`. Before this, adding
    /// `--workspace` made `hzr mcp status` report the server as unregistered and doctor
    /// treated a correctly-configured client as a missing one.
    #[test]
    fn test_a_workspace_pinned_registration_is_still_recognised_and_reported() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "[mcp_servers.hzr]\ncommand = '/opt/hzr/current/bin/hzr'\n\
             args = ['mcp', 'serve', '--workspace', '/Users/andrew/code/app']\n",
        )
        .expect("fixture");

        let status = status(Client::Codex, &path).expect("native MCP status");

        assert!(
            status.registered,
            "pinning the workspace must not look like a missing registration"
        );
        assert_eq!(
            status.pinned_workspace.as_deref(),
            Some("/Users/andrew/code/app"),
            "the pinned project must be reported so a wrong binding is visible"
        );
    }

    /// Claude Code stores its MCP servers in `~/.claude.json`, under a top-level
    /// `mcpServers` map and per-project maps. Nothing read that file, so a direct `icm`
    /// registration there — the exact thing the contract forbids — passed the ownership
    /// audit while spawning a second memory writer on every session start. Doctor saw only
    /// the resulting orphan processes and told the user to stop processes the client
    /// immediately respawns.
    #[test]
    fn test_a_direct_icm_registration_in_claude_code_is_audited() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join(".claude.json");
        fs::write(
            &path,
            r#"{
                 "mcpServers": {"icm": {"command": "/Users/andrew/.local/bin/icm", "args": ["serve"]}},
                 "projects": {
                   "/Users/andrew/code/app": {
                     "mcpServers": {"icm": {"command": "/opt/icm", "args": ["serve"]}}
                   }
                 }
               }"#,
        )
        .expect("fixture");

        let status = status(Client::ClaudeCode, &path).expect("claude code status");

        assert_eq!(
            status.direct_icm_registrations, 2,
            "both the user-scope and the project-scope registration must be counted"
        );
        assert!(!status.registered, "no hzr server is registered here");
    }

    /// Claude Code's configuration is a large user state file that HZR must never rewrite,
    /// so it belongs to the audit set and not to the set `hzr install` writes.
    #[test]
    fn test_claude_code_is_audited_but_never_written() {
        let writable = default_paths().expect("writable client paths");
        assert!(
            !writable
                .iter()
                .any(|(client, _)| *client == Client::ClaudeCode),
            "hzr install must not write Claude Code's own state file"
        );
        let audited = audit_paths().expect("audited client paths");
        assert!(
            audited
                .iter()
                .any(|(client, _)| *client == Client::ClaudeCode),
            "the ownership audit must still read it"
        );
    }

    /// The unpinned form still registers, but must be reported as unpinned so `doctor` can
    /// say that the namespace is decided by whatever directory the client launches from.
    #[test]
    fn test_an_unpinned_registration_reports_no_workspace() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "[mcp_servers.hzr]\ncommand = '/opt/hzr/current/bin/hzr'\nargs = ['mcp', 'serve']\n",
        )
        .expect("fixture");

        let status = status(Client::Codex, &path).expect("native MCP status");

        assert!(status.registered);
        assert!(status.pinned_workspace.is_none());
    }

    #[test]
    fn test_codex_migration_replaces_direct_icm_and_preserves_other_servers() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("config.toml");
        let original = "[mcp_servers.icm]\ncommand = '/usr/local/bin/icm'\nargs = ['serve']\n\n\
                        [mcp_servers.other]\ncommand = '/opt/other'\n";
        fs::write(&path, original).expect("fixture");

        let first = install(Client::Codex, &path, binary(), false, true).expect("migration");
        let after = fs::read_to_string(&path).expect("migrated config");
        let document = after.parse::<DocumentMut>().expect("valid TOML");

        assert_eq!(first.direct_icm_removed, 1);
        assert!(document["mcp_servers"].get("icm").is_none());
        assert_eq!(
            document["mcp_servers"]["hzr"]["command"].as_str(),
            Some("/opt/hzr/current/bin/hzr")
        );
        assert_eq!(
            document["mcp_servers"]["other"]["command"].as_str(),
            Some("/opt/other")
        );
        assert!(first.backup_path.expect("backup").is_file());
        assert!(
            !install(Client::Codex, &path, binary(), false, true)
                .expect("idempotent reinstall")
                .changed
        );
        let status = status(Client::Codex, &path).expect("native MCP status");
        assert!(status.registered);
        assert_eq!(status.lifecycle, MCP_LIFECYCLE);
        assert!(!status.started_by_init);
        assert_eq!(status.direct_icm_registrations, 0);
        assert_eq!(status.command.as_deref(), Some("/opt/hzr/current/bin/hzr"));
        assert_eq!(status.args, ["mcp", "serve"]);
    }

    #[test]
    fn test_project_only_migration_removes_only_the_hzr_owned_registration() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "[mcp_servers.hzr]\ncommand = '/opt/hzr/current/bin/hzr'\nargs = ['mcp', 'serve']\n\n\
             [mcp_servers.other]\ncommand = '/opt/other'\n",
        )
        .expect("fixture");

        let report = uninstall(Client::Codex, &path, false, true).expect("remove HZR MCP");
        let document = fs::read_to_string(&path)
            .expect("updated config")
            .parse::<DocumentMut>()
            .expect("valid TOML");

        assert!(report.changed);
        assert!(!report.hzr_registered);
        assert!(document["mcp_servers"].get("hzr").is_none());
        assert_eq!(
            document["mcp_servers"]["other"]["command"].as_str(),
            Some("/opt/other")
        );
    }

    #[test]
    fn test_claude_desktop_dry_run_and_migration_are_transactional() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("claude.json");
        let original = r#"{"mcpServers":{"memory":{"command":"/usr/bin/icm","args":["serve"]},"other":{"command":"/opt/other"}}}"#;
        fs::write(&path, original).expect("fixture");

        let preview = install(Client::ClaudeDesktop, &path, binary(), true, false)
            .expect("dry-run migration");
        assert!(preview.changed);
        assert_eq!(fs::read_to_string(&path).expect("unchanged"), original);

        let applied = install(Client::ClaudeDesktop, &path, binary(), false, true)
            .expect("applied migration");
        let document: Value =
            serde_json::from_slice(&fs::read(&path).expect("config bytes")).expect("valid JSON");
        let servers = document["mcpServers"].as_object().expect("servers");
        assert_eq!(applied.direct_icm_removed, 1);
        assert!(!servers.contains_key("memory"));
        assert_eq!(
            servers["hzr"]["command"],
            Value::String("/opt/hzr/current/bin/hzr".to_owned())
        );
        assert!(servers.contains_key("other"));
        let status = status(Client::ClaudeDesktop, &path).expect("native MCP status");
        assert!(status.registered);
        assert!(!status.started_by_init);
        assert_eq!(status.direct_icm_registrations, 0);
    }

    #[test]
    fn test_missing_client_config_reports_client_managed_not_started() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("missing.toml");

        let status = status(Client::Codex, &path).expect("missing config status");

        assert!(!status.config_exists);
        assert!(!status.registered);
        assert!(!status.started_by_init);
        assert_eq!(status.lifecycle, "client_managed_stdio");
    }

    #[test]
    fn test_status_rejects_non_string_native_arguments() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "[mcp_servers.hzr]\ncommand = '/opt/hzr/current/bin/hzr'\nargs = ['mcp', 1, 'serve']\n",
        )
        .expect("fixture");

        let status = status(Client::Codex, &path).expect("MCP status");

        assert!(!status.registered);
        assert!(status.args.is_empty());
    }
}
