use serde_json::Value;
use std::path::PathBuf;

/// Verdict from checking a command against Claude Code permission rules.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PermissionVerdict {
    /// An explicit allow rule matched.
    Allow,
    /// A deny rule matched.
    Deny,
    /// An ask rule matched.
    Ask,
    /// No rule matched. Treat as ask to preserve Claude Code's default.
    Default,
}

/// Check `cmd` against Claude Code Bash deny/ask/allow rules.
///
/// Precedence: Deny > Ask > Allow > Default. For compound commands, every
/// non-empty segment must independently match an allow rule before the whole
/// command can be auto-allowed.
pub fn check_command(cmd: &str) -> PermissionVerdict {
    let (deny_rules, ask_rules, allow_rules) = load_permission_rules();
    check_command_with_rules(cmd, &deny_rules, &ask_rules, &allow_rules)
}

pub(crate) fn check_command_with_rules(
    cmd: &str,
    deny_rules: &[String],
    ask_rules: &[String],
    allow_rules: &[String],
) -> PermissionVerdict {
    let segments = split_compound_command(cmd);
    let mut any_ask = false;
    let mut all_segments_allowed = true;
    let mut saw_segment = false;

    for segment in &segments {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        saw_segment = true;

        for pattern in deny_rules {
            if command_matches_pattern(segment, pattern) {
                return PermissionVerdict::Deny;
            }
        }

        if !any_ask {
            any_ask = ask_rules
                .iter()
                .any(|pattern| command_matches_pattern(segment, pattern));
        }

        if all_segments_allowed
            && !allow_rules
                .iter()
                .any(|pattern| command_matches_pattern(segment, pattern))
        {
            all_segments_allowed = false;
        }
    }

    if any_ask {
        PermissionVerdict::Ask
    } else if saw_segment && all_segments_allowed && !allow_rules.is_empty() {
        PermissionVerdict::Allow
    } else {
        PermissionVerdict::Default
    }
}

fn load_permission_rules() -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut deny_rules = Vec::new();
    let mut ask_rules = Vec::new();
    let mut allow_rules = Vec::new();

    for path in get_settings_paths() {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<Value>(&content) else {
            eprintln!(
                "[rtk] warning: failed to parse permissions from {}",
                path.display()
            );
            continue;
        };
        let Some(permissions) = json.get("permissions") else {
            continue;
        };

        append_bash_rules(permissions.get("deny"), &mut deny_rules);
        append_bash_rules(permissions.get("ask"), &mut ask_rules);
        append_bash_rules(permissions.get("allow"), &mut allow_rules);
    }

    (deny_rules, ask_rules, allow_rules)
}

fn append_bash_rules(rules_value: Option<&Value>, target: &mut Vec<String>) {
    let Some(arr) = rules_value.and_then(|v| v.as_array()) else {
        return;
    };

    for rule in arr {
        if let Some(s) = rule.as_str() {
            if s.starts_with("Bash(") {
                target.push(extract_bash_pattern(s).to_string());
            }
        }
    }
}

fn get_settings_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(root) = find_project_root() {
        paths.push(root.join(".claude").join("settings.json"));
        paths.push(root.join(".claude").join("settings.local.json"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".claude").join("settings.json"));
        paths.push(home.join(".claude").join("settings.local.json"));
    }

    paths
}

fn find_project_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".claude").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }

    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;

    if output.status.success() {
        let path = String::from_utf8(output.stdout).ok()?;
        return Some(PathBuf::from(path.trim()));
    }

    None
}

pub(crate) fn extract_bash_pattern(rule: &str) -> &str {
    if let Some(inner) = rule.strip_prefix("Bash(") {
        if let Some(pattern) = inner.strip_suffix(')') {
            return pattern;
        }
    }
    rule
}

pub(crate) fn command_matches_pattern(cmd: &str, pattern: &str) -> bool {
    let cmd_norm = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
    let pattern_norm = pattern.split_whitespace().collect::<Vec<_>>().join(" ");
    let cmd = cmd_norm.as_str();
    let pattern = pattern_norm.as_str();

    if pattern == "*" {
        return true;
    }

    if let Some(p) = pattern.strip_suffix('*') {
        let prefix = p.trim_end_matches(':').trim_end();
        if prefix.is_empty() || prefix == "*" {
            return true;
        }
        if !prefix.contains('*') {
            return cmd == prefix || cmd.starts_with(&format!("{} ", prefix));
        }
    }

    if pattern.contains('*') {
        return glob_matches(cmd, pattern);
    }

    cmd == pattern || cmd.starts_with(&format!("{} ", pattern))
}

fn glob_matches(cmd: &str, pattern: &str) -> bool {
    let normalized = pattern.replace(":*", " *").replace("*:", "* ");
    let parts: Vec<&str> = normalized.split('*').collect();

    if parts.iter().all(|p| p.is_empty()) {
        return true;
    }

    let mut search_from = 0;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if i == 0 {
            if !cmd.starts_with(part) {
                return false;
            }
            search_from = part.len();
        } else if i == parts.len() - 1 {
            if !cmd[search_from..].ends_with(*part) {
                return false;
            }
        } else {
            let remaining = &cmd[search_from..];
            if let Some(pos) = remaining.find(*part) {
                search_from += pos + part.len();
            } else {
                let trimmed = part.trim_end();
                if !trimmed.is_empty() && remaining.ends_with(trimmed) {
                    search_from += remaining.len();
                } else {
                    return false;
                }
            }
        }
    }

    true
}

fn split_compound_command(cmd: &str) -> Vec<&str> {
    let bytes = cmd.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => {
                in_single = !in_single;
                i += 1;
            }
            b'"' if !in_single => {
                in_double = !in_double;
                i += 1;
            }
            b'|' if !in_single && !in_double => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                    result.push(cmd[start..i].trim());
                    i += 2;
                    start = i;
                } else {
                    result.push(cmd[start..i].trim());
                    i += 1;
                    start = i;
                }
            }
            b'&' if !in_single && !in_double && i + 1 < bytes.len() && bytes[i + 1] == b'&' => {
                result.push(cmd[start..i].trim());
                i += 2;
                start = i;
            }
            b';' if !in_single && !in_double => {
                result.push(cmd[start..i].trim());
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }

    result.push(cmd[start..].trim());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bash_permission_rule() {
        assert_eq!(
            extract_bash_pattern("Bash(git push --force)"),
            "git push --force"
        );
        assert_eq!(extract_bash_pattern("Read(**/.env*)"), "Read(**/.env*)");
    }

    #[test]
    fn default_is_not_allow() {
        assert_eq!(
            check_command_with_rules("git status", &[], &[], &[]),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn deny_precedes_ask_and_allow() {
        let deny = vec!["git push --force".to_string()];
        let ask = vec!["git push".to_string()];
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git push --force origin main", &deny, &ask, &allow),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn ask_precedes_allow() {
        let ask = vec!["git push".to_string()];
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git push origin main", &[], &ask, &allow),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn all_compound_segments_must_be_allowed() {
        let allow = vec!["git status".to_string()];
        assert_eq!(
            check_command_with_rules("git status && git add .", &[], &[], &allow),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn compound_allows_when_every_segment_matches() {
        let allow = vec!["git *".to_string(), "cargo *".to_string()];
        assert_eq!(
            check_command_with_rules("git status && cargo test", &[], &[], &allow),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn quoted_operators_are_not_split() {
        let deny = vec!["git push --force".to_string()];
        assert_eq!(
            check_command_with_rules(r#"echo "git push --force && danger""#, &deny, &[], &[]),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn complex_wildcards_match() {
        assert!(command_matches_pattern("git push --force", "* --force"));
        assert!(command_matches_pattern(
            "git -C /tmp/repo diff",
            "git -C * diff:*"
        ));
        assert!(!command_matches_pattern("git push --forceful", "* --force"));
    }

    #[test]
    fn extra_whitespace_still_matches() {
        assert!(command_matches_pattern("git  push", "git push"));
        assert!(command_matches_pattern("git\tpush origin", "git push"));
        assert!(command_matches_pattern(
            "git   push   --force",
            "git push --force"
        ));
    }

    #[test]
    fn extra_whitespace_does_not_evade_deny() {
        let deny = vec!["git push".to_string()];
        assert_eq!(
            check_command_with_rules("git  push origin main", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn whitespace_normalization_preserves_word_boundaries() {
        assert!(!command_matches_pattern(
            "git  push  --forceful",
            "git push --force"
        ));
        assert!(!command_matches_pattern("sudoedit /etc/hosts", "sudo:*"));
    }
}
