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
}

impl Client {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeDesktop => "claude-desktop",
        }
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
    pub lifecycle: &'static str,
    pub started_by_init: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Registration {
    command: String,
    args: Vec<String>,
}

impl Registration {
    fn is_native_hzr(&self) -> bool {
        Path::new(&self.command)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "hzr")
            && self.args == ["mcp", "serve"]
    }

    fn matches(&self, binary: &Path) -> bool {
        self.command == binary.to_string_lossy() && self.args == ["mcp", "serve"]
    }
}

pub const MCP_LIFECYCLE: &str = "client_managed_stdio";

pub fn default_paths() -> Result<Vec<(Client, PathBuf)>> {
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

pub fn status_all() -> Result<Vec<ClientMcpStatus>> {
    default_paths()?
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
        Client::ClaudeDesktop => {
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
        lifecycle: MCP_LIFECYCLE,
        started_by_init: false,
    })
}

pub fn direct_icm_registrations() -> Result<Vec<String>> {
    let mut found = Vec::new();
    for (client, path) in default_paths()? {
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
            Client::ClaudeDesktop => {
                let document: Value = serde_json::from_slice(&bytes)
                    .with_context(|| format!("failed to parse {}", path.display()))?;
                json_direct_icm_count(&document)
            }
        };
        if count > 0 {
            found.push(format!(
                "{} ({}, {} registration(s))",
                client.as_str(),
                path.display(),
                count
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

fn json_servers(document: &Value) -> Option<&Map<String, Value>> {
    document.get("mcpServers").and_then(Value::as_object)
}

fn json_direct_icm_count(document: &Value) -> usize {
    json_servers(document)
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::Value;
    use tempfile::tempdir;
    use toml_edit::DocumentMut;

    use super::{Client, MCP_LIFECYCLE, install, status};

    fn binary() -> &'static Path {
        Path::new("/opt/hzr/current/bin/hzr")
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
