use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jsonc_parser::cst::{CstInputValue, CstRootNode};
use serde::Serialize;
use serde_json::Value;

use super::{Client, audit_paths, client_state_paths, opencode};
use crate::adoption::{commit_with_lock, read_optional, validate_lifecycle_target};

#[derive(Clone, Debug, Serialize)]
pub struct ClientOwnershipRepair {
    pub path: PathBuf,
    pub disabled_registrations: usize,
    pub backup_path: Option<PathBuf>,
    pub dry_run: bool,
    pub stopped_processes: Vec<u32>,
}

pub fn repair_opencode(workspace: &Path, dry_run: bool) -> Result<Vec<ClientOwnershipRepair>> {
    let mut paths = audit_paths()?
        .into_iter()
        .filter_map(|(client, path)| (client == Client::Opencode).then_some(path))
        .chain(opencode::project_paths(workspace))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    let mut reports = Vec::new();
    for path in paths {
        validate_lifecycle_target(&path)?;
        let before = read_optional(&path)?;
        if before.is_empty() {
            continue;
        }
        let (after, launches, disabled) = disable_direct_icm(&path, &before)?;
        if launches.is_empty() {
            continue;
        }
        let backup_path = if disabled > 0 {
            let (backup, lock) = client_state_paths(&path, &before)?;
            if !dry_run {
                commit_with_lock(&path, &before, after.as_bytes(), &backup, b"", &lock)?;
            }
            Some(backup)
        } else {
            None
        };
        let stopped_processes = if dry_run {
            Vec::new()
        } else {
            crate::foreign::stop_disabled_opencode(&launches)?
        };
        reports.push(ClientOwnershipRepair {
            path,
            disabled_registrations: disabled,
            backup_path,
            dry_run,
            stopped_processes,
        });
    }
    Ok(reports)
}

fn disable_direct_icm(path: &Path, before: &[u8]) -> Result<(String, Vec<Vec<String>>, usize)> {
    opencode::registration_status(path, before)?;
    let text = std::str::from_utf8(before)?;
    let root = CstRootNode::parse(text, &Default::default())?;
    let object = root
        .object_value()
        .context("opencode config must be an object")?;
    let Some(mcp) = object.object_value("mcp") else {
        return Ok((text.to_owned(), Vec::new(), 0));
    };
    let v2 = mcp.get("servers").is_some();
    let servers = if v2 {
        mcp.object_value("servers").context("MCP servers object")?
    } else {
        mcp
    };
    let mut launches = Vec::new();
    let mut disabled = 0;
    for property in servers.properties() {
        let name = property
            .name()
            .context("MCP server name")?
            .decoded_value()?;
        let Some(server) = servers.object_value(&name) else {
            continue;
        };
        let value: Value =
            jsonc_parser::parse_to_serde_value(&server.to_string(), &Default::default())?
                .context("MCP server must contain an object")?;
        if value.get("type").and_then(Value::as_str) != Some("local") {
            continue;
        }
        let Some(command) = value.get("command").and_then(Value::as_array) else {
            continue;
        };
        let Some(command) = command
            .iter()
            .map(|word| word.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        if !command
            .first()
            .is_some_and(|program| super::command_launches_icm(program))
        {
            continue;
        }
        launches.push(command);
        let (field, desired) = if v2 {
            ("disabled", true)
        } else {
            ("enabled", false)
        };
        if value.get(field).and_then(Value::as_bool) == Some(desired) {
            continue;
        }
        match server.get(field) {
            Some(property) => property.set_value(CstInputValue::Bool(desired)),
            None => {
                server.append(field, CstInputValue::Bool(desired));
            }
        }
        disabled += 1;
    }
    let after = root.to_string();
    anyhow::ensure!(
        opencode::registration_status(path, after.as_bytes())?.1 == 0,
        "OpenCode ownership repair did not disable every direct ICM registration"
    );
    Ok((after, launches, disabled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_disables_only_icm_preserves_comments_and_is_idempotent() {
        let source = r#"{
            // keep this user's comment
            "model": "keep",
            "mcp": {
                "memory": {"type":"local","command":["/opt/icm","serve"],"enabled":true},
                "other": {"type":"local","command":["other","serve"]},
            }
        }"#;
        let path = Path::new("opencode.jsonc");
        let (after, launches, disabled) =
            disable_direct_icm(path, source.as_bytes()).expect("repair");
        assert_eq!(disabled, 1);
        assert_eq!(launches, [vec!["/opt/icm".to_owned(), "serve".to_owned()]]);
        assert!(after.contains("// keep this user's comment"));
        assert!(after.contains("\"model\": \"keep\""));
        assert!(
            after.contains("\"other\": {\"type\":\"local\",\"command\":[\"other\",\"serve\"]}")
        );
        let (again, _, disabled) = disable_direct_icm(path, after.as_bytes()).expect("repeat");
        assert_eq!(again, after);
        assert_eq!(disabled, 0);
    }

    #[test]
    fn repair_v2_adds_disabled_and_rejects_malformed_input() {
        let path = Path::new("opencode.json");
        let (after, _, count) = disable_direct_icm(
            path,
            br#"{"mcp":{"servers":{"icm":{"type":"local","command":["icm","serve"]}}}}"#,
        )
        .expect("v2");
        assert_eq!(count, 1);
        let value: Value = serde_json::from_str(&after).expect("JSON");
        assert_eq!(value["mcp"]["servers"]["icm"]["disabled"], true);
        assert!(disable_direct_icm(path, b"{broken").is_err());
    }
}
