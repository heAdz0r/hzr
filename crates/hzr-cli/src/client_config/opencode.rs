use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::{Registration, command_launches_icm};

pub(super) const REMEDIATION: &str = "Run `hzr doctor --fix --dry-run` to preview and `hzr doctor --fix` to disable the direct ICM \
     entry and stop its verified opencode child; to use HZR, register a local MCP command \
     [\"hzr\", \"mcp\", \"serve\", \"--workspace\", \"<dir>\"] in that project's config";

pub(super) fn global_paths(home: &Path) -> Vec<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let mut paths = config_paths(&config.join("opencode"));
    if let Some(directory) = std::env::var_os("OPENCODE_CONFIG_DIR") {
        paths.extend(config_paths(Path::new(&directory)));
    }
    if let Some(path) = std::env::var_os("OPENCODE_CONFIG") {
        paths.push(PathBuf::from(path));
    }
    paths.sort();
    paths.dedup();
    paths
}

fn config_paths(directory: &Path) -> Vec<PathBuf> {
    ["opencode.json", "opencode.jsonc"]
        .into_iter()
        .map(|name| directory.join(name))
        .collect()
}

pub(super) fn project_paths(workspace: &Path) -> Vec<PathBuf> {
    workspace
        .ancestors()
        .flat_map(|directory| {
            config_paths(directory)
                .into_iter()
                .chain(config_paths(&directory.join(".opencode")))
        })
        .collect()
}

fn parse_jsonc(text: &str) -> Result<Value> {
    use jsonc_parser::tokens::{Token, TokenAndRange};

    let parsed = jsonc_parser::parse_to_ast(
        text,
        &jsonc_parser::CollectOptions {
            comments: jsonc_parser::CommentCollectionStrategy::AsTokens,
            tokens: true,
        },
        &jsonc_parser::ParseOptions {
            allow_comments: true,
            allow_trailing_commas: true,
            allow_loose_object_property_names: false,
        },
    )?;
    // The MSRV-compatible parser also accepts JSON5 syntax and missing commas.
    // Remove only tokenized JSONC extensions, then let serde enforce strict JSON.
    let mut json = text.as_bytes().to_vec();
    let mut previous: Option<TokenAndRange<'_>> = None;
    for token in parsed.tokens.context("JSONC token collection missing")? {
        if matches!(token.token, Token::CommentLine(_) | Token::CommentBlock(_)) {
            json[token.range.start..token.range.end].fill(b' ');
            continue;
        }
        if matches!(token.token, Token::CloseBrace | Token::CloseBracket) {
            if let Some(previous) = &previous {
                if previous.token == Token::Comma {
                    json[previous.range.start..previous.range.end].fill(b' ');
                }
            }
        }
        previous = Some(token);
    }
    Ok(serde_json::from_slice(&json)?)
}

pub(super) fn registration_status(
    path: &Path,
    bytes: &[u8],
) -> Result<(Option<Registration>, usize)> {
    if bytes.is_empty() {
        return Ok((None, 0));
    }
    let text =
        std::str::from_utf8(bytes).with_context(|| format!("{} is not UTF-8", path.display()))?;
    let document = parse_jsonc(text)
        .with_context(|| format!("failed to parse opencode config {}", path.display()))?;
    let document = document
        .as_object()
        .with_context(|| format!("opencode config {} must be an object", path.display()))?;
    let Some(mcp) = document.get("mcp") else {
        return Ok((None, 0));
    };
    let mcp = mcp.as_object().context("opencode mcp must be an object")?;
    // V1 stores names directly under mcp; V2 nests them under mcp.servers.
    let servers = match mcp.get("servers") {
        Some(servers) => servers
            .as_object()
            .context("opencode mcp.servers must be an object")?,
        None => mcp,
    };
    let direct = servers
        .values()
        .filter(|server| {
            active_local(server)
                && server
                    .get("command")
                    .and_then(Value::as_array)
                    .and_then(|command| command.first())
                    .and_then(Value::as_str)
                    .is_some_and(command_launches_icm)
        })
        .count();
    let registration = servers
        .get("hzr")
        .filter(|server| active_local(server))
        .and_then(|server| {
            let command = server.get("command")?.as_array()?;
            let (program, args) = command.split_first()?;
            Some(Registration {
                command: program.as_str()?.to_owned(),
                args: args
                    .iter()
                    .map(|arg| arg.as_str().map(str::to_owned))
                    .collect::<Option<_>>()?,
            })
        });
    Ok((registration, direct))
}

fn active_local(server: &Value) -> bool {
    server.get("type").and_then(Value::as_str) == Some("local")
        && server.get("enabled").and_then(Value::as_bool) != Some(false)
        && server.get("disabled").and_then(Value::as_bool) != Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_config::{Client, default_paths, install, status};

    #[test]
    fn opencode_jsonc_audits_active_array_commands_without_writing() {
        let directory = tempfile::tempdir().expect("fixture");
        let path = directory.path().join("opencode.jsonc");
        let original = r#"{
            // User configuration must remain byte-exact.
            "model": "keep/me",
            "mcp": {
                "memory": {"type":"local","command":["/opt/icm","serve"],"enabled":true},
                "disabled": {"type":"local","command":["icm","serve"],"enabled":false},
                "remote": {"type":"remote","url":"https://example.com/icm"},
                "hzr": {"type":"local","command":["/opt/hzr","mcp","serve","--workspace","/project"]},
            },
        }"#;
        std::fs::write(&path, original).expect("config");
        let report = status(Client::Opencode, &path).expect("audit");
        assert_eq!(report.direct_icm_registrations, 1);
        let findings =
            super::super::direct_icm_registrations_at(vec![(Client::Opencode, path.clone())])
                .expect("ownership findings");
        assert_eq!(findings.len(), 1);
        assert!(findings[0].contains(path.to_string_lossy().as_ref()));
        assert!(findings[0].contains("doctor --fix"));
        assert!(report.registered);
        assert_eq!(report.pinned_workspace.as_deref(), Some("/project"));
        assert!(
            install(
                Client::Opencode,
                &path,
                Path::new("/opt/hzr"),
                directory.path(),
                false,
                true
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).expect("unchanged"), original);
        assert!(
            default_paths()
                .expect("paths")
                .iter()
                .all(|(client, _)| *client != Client::Opencode)
        );
    }

    #[test]
    fn opencode_v2_and_disabled_hzr_are_recognized() {
        let (registration, count) = registration_status(
            Path::new("opencode.json"),
            br#"{
            "mcp":{"servers":{
                "memory":{"type":"local","command":["icm","serve"]},
                "off":{"type":"local","command":["icm","serve"],"disabled":true},
                "hzr":{"type":"local","command":["hzr","mcp","serve"],"disabled":true}
            }}
        }"#,
        )
        .expect("v2");
        assert_eq!(count, 1);
        assert!(registration.is_none());
    }

    #[test]
    fn project_discovery_covers_root_hidden_jsonc_and_ancestors() {
        let directory = tempfile::tempdir().expect("fixture");
        let workspace = directory.path().join("app");
        let paths = project_paths(&workspace);
        for path in [
            workspace.join("opencode.json"),
            workspace.join(".opencode/opencode.jsonc"),
            directory.path().join("opencode.jsonc"),
        ] {
            assert!(paths.contains(&path), "missing {}", path.display());
        }
    }

    #[test]
    fn malformed_opencode_config_is_not_a_clean_audit() {
        for bytes in [
            b"{broken".as_slice(),
            b"  ",
            b"{'mcp':{}}",
            b"{\"value\":0xff}",
            b"{\"value\":+1}",
            b"{\"first\":1 \"second\":2}",
            b"[]",
            b"{,}",
            b"[,]",
            b"{\"value\":1,,}",
            b"{\"mcp\":[]}",
            b"{\"mcp\":{\"servers\":false}}",
        ] {
            assert!(
                registration_status(Path::new("opencode.jsonc"), bytes).is_err(),
                "accepted malformed input: {bytes:?}"
            );
        }
    }
}
