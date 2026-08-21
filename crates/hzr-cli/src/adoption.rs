use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use directories::BaseDirs;
use fs2::FileExt;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const HZR_DISPATCH_SUFFIX: &str = " hooks dispatch";
const HZR_OBSERVE_SUFFIX: &str = " hooks observe";
const HZR_FEEDBACK_SUFFIX: &str = " hooks feedback";
const HZR_INIT_SUFFIX: &str = " init --if-needed --quiet --session-start-hook";
const HZR_INIT_SKIP_SERVICE_SUFFIX: &str =
    " init --if-needed --quiet --session-start-hook --skip-service";
const HZR_INIT_ENABLED_SUFFIX: &str = " init --if-enabled --quiet --session-start-hook";
const HZR_INIT_ENABLED_SKIP_SERVICE_SUFFIX: &str =
    " init --if-enabled --quiet --session-start-hook --skip-service";

/// What "settings.json is absent" means, so a first install is not mistaken for a
/// concurrent modification during compare-and-swap.
const SETTINGS_MISSING_DEFAULT: &[u8] = b"{}\n";

/// Build directories. A hook command pointing here breaks the moment the tree is
/// cleaned or the bundle directory is removed, so installing from one is refused.
const DEV_PATH_MARKERS: [&str; 2] = ["/target/debug/", "/target/release/"];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum NativeToolMode {
    Observe,
    Steer,
    Strict,
}

impl NativeToolMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Steer => "steer",
            Self::Strict => "strict",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HookInstallPolicy {
    pub native_tool_mode: Option<NativeToolMode>,
    pub dry_run: bool,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HookStatus {
    pub settings_path: PathBuf,
    pub hzr_entries: usize,
    pub rtk_entries: usize,
    pub external_icm_entries: usize,
    pub installed: bool,
    pub conflict: bool,
    pub native_tool_mode: Option<NativeToolMode>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AdoptionReport {
    pub action: &'static str,
    pub dry_run: bool,
    pub changed: bool,
    pub settings_path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub before_sha256: String,
    pub after_sha256: String,
    pub status: HookStatus,
    #[serde(skip)]
    pub rendered_settings: String,
}

pub fn default_settings_path() -> Result<PathBuf> {
    if let Some(directory) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Ok(PathBuf::from(directory).join("settings.json"));
    }
    let base = BaseDirs::new().context("cannot determine the user home directory")?;
    Ok(base.home_dir().join(".claude/settings.json"))
}

pub fn status(path: &Path) -> Result<HookStatus> {
    let bytes = read_settings(path)?;
    let document = parse_settings(path, &bytes)?;
    Ok(classify(path, &document))
}

pub fn install(
    path: &Path,
    binary: &Path,
    adopt_icm: bool,
    start_service: bool,
    project_only: bool,
    policy: HookInstallPolicy,
) -> Result<AdoptionReport> {
    let HookInstallPolicy {
        native_tool_mode,
        dry_run,
        confirmed,
    } = policy;
    let before = read_settings(path)?;
    let mut document = parse_settings(path, &before)?;
    let native_tool_mode = native_tool_mode
        .or_else(|| installed_native_tool_mode(&document))
        .unwrap_or(NativeToolMode::Steer);
    remove_owned_hooks(&mut document, HookOwner::Rtk);
    remove_owned_hooks(&mut document, HookOwner::Hzr);
    if adopt_icm {
        // Centralized memory ownership (PRD §6.5): a direct `icm hook` writes to a
        // store HZR does not supervise, which is a second memory layer. Removing it
        // makes `hzr memory` the only durable path. Off by default would leave the
        // duplicate in place, so callers opt out explicitly instead.
        remove_owned_hooks(&mut document, HookOwner::Icm);
    }
    add_hzr_hooks(
        &mut document,
        &managed_commands(binary, start_service, project_only, native_tool_mode)?,
    )?;
    let after = render_settings(&document)?;
    let changed = before != after.as_bytes();
    let backup_path = changed.then(|| backup_path(path, &before));

    if changed && !dry_run {
        if !confirmed {
            bail!(
                "installation changes {}; inspect `hzr install --dry-run`, then rerun with `--force` to confirm",
                path.display()
            );
        }
        let backup = backup_path
            .as_ref()
            .context("changed installation must have a backup path")?;
        commit(
            path,
            &before,
            after.as_bytes(),
            backup,
            SETTINGS_MISSING_DEFAULT,
        )?;
    }

    Ok(report(ReportParts {
        action: "install",
        path,
        dry_run,
        changed,
        backup_path,
        before: &before,
        rendered_settings: after,
        document,
    }))
}

pub fn uninstall(path: &Path, dry_run: bool, confirmed: bool) -> Result<AdoptionReport> {
    let before = read_settings(path)?;
    let mut document = parse_settings(path, &before)?;
    remove_owned_hooks(&mut document, HookOwner::Hzr);
    let after = render_settings(&document)?;
    let changed = before != after.as_bytes();
    let backup_path = changed.then(|| backup_path(path, &before));

    if changed && !dry_run {
        if !confirmed {
            bail!(
                "uninstallation changes {}; inspect with `hzr uninstall --dry-run`, then rerun with `--force` to confirm",
                path.display()
            );
        }
        let backup = backup_path
            .as_ref()
            .context("changed uninstallation must have a backup path")?;
        commit(
            path,
            &before,
            after.as_bytes(),
            backup,
            SETTINGS_MISSING_DEFAULT,
        )?;
    }

    Ok(report(ReportParts {
        action: "uninstall",
        path,
        dry_run,
        changed,
        backup_path,
        before: &before,
        rendered_settings: after,
        document,
    }))
}

struct ReportParts<'a> {
    action: &'static str,
    path: &'a Path,
    dry_run: bool,
    changed: bool,
    backup_path: Option<PathBuf>,
    before: &'a [u8],
    rendered_settings: String,
    document: Value,
}

fn report(parts: ReportParts<'_>) -> AdoptionReport {
    AdoptionReport {
        action: parts.action,
        dry_run: parts.dry_run,
        changed: parts.changed,
        settings_path: parts.path.to_path_buf(),
        backup_path: parts.backup_path,
        before_sha256: sha256(parts.before),
        after_sha256: sha256(parts.rendered_settings.as_bytes()),
        status: classify(parts.path, &parts.document),
        rendered_settings: parts.rendered_settings,
    }
}

fn read_settings(path: &Path) -> Result<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(b"{}\n".to_vec()),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Read a file that may legitimately not exist yet, returning empty bytes instead
/// of a JSON stub. Used for instruction files, which have no default document.
pub(crate) fn read_optional(path: &Path) -> Result<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn parse_settings(path: &Path, bytes: &[u8]) -> Result<Value> {
    let document: Value = serde_json::from_slice(bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if !document.is_object() {
        bail!("{} must contain a JSON object", path.display());
    }
    Ok(document)
}

fn render_settings(document: &Value) -> Result<String> {
    let mut rendered = serde_json::to_string_pretty(document)?;
    rendered.push('\n');
    Ok(rendered)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookOwner {
    Hzr,
    Rtk,
    Icm,
    Other,
}

fn owner(command: &str) -> HookOwner {
    if hzr_dispatch_mode(command).is_some()
        || hzr_observe_mode(command).is_some()
        || command.trim_end().ends_with(HZR_FEEDBACK_SUFFIX)
        || command.trim_end().ends_with(HZR_INIT_SUFFIX)
        || command.trim_end().ends_with(HZR_INIT_SKIP_SERVICE_SUFFIX)
        || command.trim_end().ends_with(HZR_INIT_ENABLED_SUFFIX)
        || command
            .trim_end()
            .ends_with(HZR_INIT_ENABLED_SKIP_SERVICE_SUFFIX)
    {
        HookOwner::Hzr
    } else if command.contains("rtk-rewrite.sh")
        || command.contains("rtk-mem-context.sh")
        || command.contains("rtk-block-native-explore.sh")
        || command.trim_start().starts_with("rtk hook ")
    {
        HookOwner::Rtk
    } else if command.contains("icm hook ") {
        HookOwner::Icm
    } else {
        HookOwner::Other
    }
}

fn classify(path: &Path, document: &Value) -> HookStatus {
    let mut hzr_entries = 0;
    let mut rtk_entries = 0;
    let mut external_icm_entries = 0;
    visit_commands(document, &mut |command| match owner(command) {
        HookOwner::Hzr => hzr_entries += 1,
        HookOwner::Rtk => rtk_entries += 1,
        HookOwner::Icm => external_icm_entries += 1,
        HookOwner::Other => {}
    });
    HookStatus {
        settings_path: path.to_path_buf(),
        hzr_entries,
        rtk_entries,
        external_icm_entries,
        installed: hzr_entries == 6 && rtk_entries == 0,
        conflict: hzr_entries > 0 && rtk_entries > 0,
        native_tool_mode: installed_native_tool_mode(document),
    }
}

fn installed_native_tool_mode(document: &Value) -> Option<NativeToolMode> {
    let mut mode = None;
    visit_commands(document, &mut |command| {
        mode = hzr_dispatch_mode(command).or(mode);
    });
    mode
}

fn hzr_dispatch_mode(command: &str) -> Option<NativeToolMode> {
    hzr_native_mode(command, HZR_DISPATCH_SUFFIX)
}

fn hzr_observe_mode(command: &str) -> Option<NativeToolMode> {
    hzr_native_mode(command, HZR_OBSERVE_SUFFIX)
}

fn hzr_native_mode(command: &str, command_suffix: &str) -> Option<NativeToolMode> {
    let command = command.trim_end();
    for mode in [
        NativeToolMode::Observe,
        NativeToolMode::Steer,
        NativeToolMode::Strict,
    ] {
        let suffix = format!("{command_suffix} --native-mode {}", mode.as_str());
        if command.ends_with(&suffix) {
            return Some(mode);
        }
    }
    // A pre-enforcement dispatcher is an existing installation. Preserve its behaviour
    // on upgrade and let doctor advertise the explicit opt-in.
    command
        .ends_with(command_suffix)
        .then_some(NativeToolMode::Observe)
}

fn visit_commands(value: &Value, visit: &mut impl FnMut(&str)) {
    match value {
        Value::Object(object) => {
            if let Some(command) = object.get("command").and_then(Value::as_str) {
                visit(command);
            }
            for value in object.values() {
                visit_commands(value, visit);
            }
        }
        Value::Array(values) => {
            for value in values {
                visit_commands(value, visit);
            }
        }
        _ => {}
    }
}

fn remove_owned_hooks(document: &mut Value, target: HookOwner) {
    let Some(events) = document.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    events.retain(|_, entries| {
        let Some(entries) = entries.as_array_mut() else {
            return true;
        };
        entries.retain_mut(|entry| {
            let Some(hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            hooks.retain(|hook| {
                hook.get("command")
                    .and_then(Value::as_str)
                    .is_none_or(|command| owner(command) != target)
            });
            !hooks.is_empty()
        });
        !entries.is_empty()
    });
}

struct ManagedCommands {
    dispatch: String,
    observe: String,
    feedback: String,
    init: String,
}

/// Resolve the executable that installed hooks will invoke.
///
/// `current_exe()` alone is wrong: running `hzr install` from `cargo run` or from an
/// unpacked bundle would pin the hook to `target/debug/hzr` or a temporary directory,
/// and every Bash command breaks once that path disappears. The hook must name a
/// durable location, so an explicit `--binary` wins, otherwise the resolved
/// `current_exe()` is accepted only when it is not a build directory.
pub fn resolve_hook_binary(
    explicit: Option<&Path>,
    allow_dev_path: bool,
    allow_missing: bool,
) -> Result<PathBuf> {
    let candidate = match explicit {
        Some(path) => path.to_path_buf(),
        None => std::env::current_exe().context("cannot resolve the HZR executable")?,
    };
    let durable = if candidate.is_absolute() {
        candidate.clone()
    } else {
        std::env::current_dir()
            .context("cannot resolve the current directory")?
            .join(&candidate)
    };
    // Resolve the physical target only for validation. Persisting this canonical path
    // would turn ~/.local/bin/hzr into versions/<release>/bin/hzr and freeze every hook
    // and MCP client on the release that happened to run the installer.
    let physical = match durable.canonicalize() {
        Ok(resolved) => resolved,
        Err(error) if allow_missing && error.kind() == std::io::ErrorKind::NotFound => {
            durable.clone()
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("HZR executable does not exist: {}", durable.display()));
        }
    };
    let display = physical.to_string_lossy().replace('\\', "/");
    if !allow_dev_path
        && DEV_PATH_MARKERS
            .iter()
            .any(|marker| display.contains(marker))
    {
        bail!(
            "refusing to bind hooks to the build directory {}: install a durable binary \
             (`hzr install --prefix ~/.local/bin`) or pass `--binary <path>`; \
             `--allow-dev-path` overrides this for development only",
            physical.display()
        );
    }
    Ok(durable)
}

fn managed_commands(
    binary: &Path,
    start_service: bool,
    project_only: bool,
    native_tool_mode: NativeToolMode,
) -> Result<ManagedCommands> {
    let executable = shell_word(binary)?;
    let init_suffix = match (project_only, start_service) {
        (true, true) => HZR_INIT_ENABLED_SUFFIX,
        (true, false) => HZR_INIT_ENABLED_SKIP_SERVICE_SUFFIX,
        (false, true) => HZR_INIT_SUFFIX,
        (false, false) => HZR_INIT_SKIP_SERVICE_SUFFIX,
    };
    Ok(ManagedCommands {
        dispatch: format!(
            "{executable}{HZR_DISPATCH_SUFFIX} --native-mode {}",
            native_tool_mode.as_str()
        ),
        observe: format!(
            "{executable}{HZR_OBSERVE_SUFFIX} --native-mode {}",
            native_tool_mode.as_str()
        ),
        feedback: format!("{executable}{HZR_FEEDBACK_SUFFIX}"),
        init: format!("{executable}{init_suffix}"),
    })
}

fn shell_word(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .with_context(|| format!("HZR executable path is not UTF-8: {}", path.display()))?;
    Ok(format!("'{}'", path.replace('\'', "'\\''")))
}

fn add_hzr_hooks(document: &mut Value, commands: &ManagedCommands) -> Result<()> {
    let root = document
        .as_object_mut()
        .context("settings document must remain a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("settings hooks must be a JSON object")?;
    append_event(
        hooks,
        "SessionStart",
        json!({
            "hooks": [{"type": "command", "command": commands.init, "timeout": 10}]
        }),
    )?;
    append_event(
        hooks,
        "PreToolUse",
        json!({
            "matcher": "Bash|Agent|Task|Read|Grep|Glob|Edit|Write",
            "hooks": [{"type": "command", "command": commands.dispatch, "timeout": 10}]
        }),
    )?;
    append_event(
        hooks,
        "PostToolUse",
        json!({
            "matcher": "Read|Grep|Glob|Edit|Write",
            "hooks": [{"type": "command", "command": commands.observe, "timeout": 10}]
        }),
    )?;
    append_event(
        hooks,
        "UserPromptSubmit",
        json!({
            "hooks": [{"type": "command", "command": commands.feedback, "timeout": 10}]
        }),
    )?;
    for event in ["Stop", "SubagentStop"] {
        append_event(
            hooks,
            event,
            json!({
                "hooks": [{"type": "command", "command": commands.feedback, "timeout": 10}]
            }),
        )?;
    }
    Ok(())
}

fn append_event(hooks: &mut Map<String, Value>, event: &str, entry: Value) -> Result<()> {
    let entries = hooks
        .entry(event)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .with_context(|| format!("settings hook event {event} must be an array"))?;
    entries.push(entry);
    Ok(())
}

pub(crate) fn backup_path(path: &Path, bytes: &[u8]) -> PathBuf {
    path.with_file_name(format!(
        "{}.hzr-backup-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("settings.json"),
        sha256(bytes)
    ))
}

fn retain_backup(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        let retained = fs::read(path)
            .with_context(|| format!("failed to verify retained backup {}", path.display()))?;
        if retained != bytes {
            bail!("retained backup has unexpected content: {}", path.display());
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create backup {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn commit(
    path: &Path,
    before: &[u8],
    after: &[u8],
    backup: &Path,
    missing_default: &[u8],
) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("settings path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    // Lock name derives from the target file so CLAUDE.md and settings.json never
    // contend on one lock while still serialising writers to the same file.
    let lock_path = parent.join(format!(
        "{}.hzr.lock",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("settings.json")
    ));
    let mut lock_options = OpenOptions::new();
    lock_options
        .create(true)
        .truncate(false)
        .read(true)
        .write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        lock_options.mode(0o600);
    }
    let lock = lock_options
        .open(&lock_path)
        .with_context(|| format!("failed to open {}", lock_path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("failed to lock {}", lock_path.display()))?;

    // Compare-and-swap under the lock. `missing_default` is what "file absent" means
    // for this file type: a JSON stub for settings.json, empty for instruction files.
    // Using the wrong default would either reject a valid first install or silently
    // overwrite a file created between plan and commit.
    let current = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing_default.to_vec(),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    if current != before {
        bail!(
            "{} changed after the installation plan was computed; retry",
            path.display()
        );
    }
    retain_backup(backup, before)?;
    atomic_write(path, after)?;
    FileExt::unlock(&lock).with_context(|| format!("failed to unlock {}", lock_path.display()))?;
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("settings path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create temporary settings in {}",
            parent.display()
        )
    })?;
    temporary.write_all(bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use std::path::Path;

    use super::{
        HookInstallPolicy, NativeToolMode, install, resolve_hook_binary, status, uninstall,
    };

    /// Stable stand-in for the durable installed binary.
    fn binary() -> &'static Path {
        Path::new("/opt/hzr/bin/hzr")
    }

    fn policy(dry_run: bool, confirmed: bool) -> HookInstallPolicy {
        HookInstallPolicy {
            native_tool_mode: None,
            dry_run,
            confirmed,
        }
    }

    #[test]
    fn install_replaces_rtk_once_and_adopts_external_icm() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    {"matcher": "Bash", "hooks": [
                        {"type": "command", "command": "/home/u/.claude/hooks/rtk-rewrite.sh"},
                        {"type": "command", "command": "/home/u/bin/foreign-hook"}
                    ]},
                    {"matcher": "Task", "hooks": [{"type": "command", "command": "/home/u/.claude/hooks/rtk-mem-context.sh"}]}
                ],
                "UserPromptSubmit": [
                    {"hooks": [{"type": "command", "command": "/home/u/.local/bin/icm hook prompt"}]}
                ]
            }
        });
        fs::write(
            &path,
            serde_json::to_vec_pretty(&initial).expect("settings JSON"),
        )
        .expect("settings write");

        let first = install(&path, binary(), true, true, false, policy(false, true))
            .expect("first install");
        let second = install(&path, binary(), true, true, false, policy(false, true))
            .expect("idempotent install");
        let current = status(&path).expect("hook status");

        assert!(first.changed);
        assert!(!second.changed);
        assert_eq!(current.hzr_entries, 6);
        assert_eq!(current.rtk_entries, 0);
        assert_eq!(
            current.external_icm_entries, 0,
            "centralized memory ownership removes the direct ICM hook"
        );
        assert!(current.installed);
        let installed = fs::read_to_string(&path).expect("installed settings");
        assert!(installed.contains("PostToolUse"));
        assert!(installed.contains("Read|Grep|Glob|Edit|Write"));
        assert!(installed.contains("hooks observe --native-mode steer"));
        assert!(installed.contains("hooks dispatch --native-mode steer"));
        assert!(installed.contains("UserPromptSubmit"));
        assert!(installed.contains("SubagentStop"));
        assert_eq!(current.native_tool_mode, Some(NativeToolMode::Steer));
        assert!(
            installed.contains("/home/u/bin/foreign-hook"),
            "unknown third-party handlers must survive adoption"
        );
        assert!(
            installed.contains("/opt/hzr/bin/hzr"),
            "hooks must name the durable installed binary"
        );
        let backup = first.backup_path.expect("backup path");
        assert_eq!(
            fs::read(backup).expect("backup bytes"),
            serde_json::to_vec_pretty(&initial).expect("settings JSON")
        );
    }

    #[test]
    fn acceptance_gate_upgrade_observes_while_new_install_steers() {
        let directory = tempdir().expect("temporary directory");
        let legacy_path = directory.path().join("legacy.json");
        fs::write(
            &legacy_path,
            serde_json::to_vec_pretty(&json!({
                "hooks": {"PreToolUse": [{"matcher": "Bash|Agent|Task", "hooks": [
                    {"type": "command", "command": "'/opt/hzr/bin/hzr' hooks dispatch"}
                ]}]}
            }))
            .expect("legacy settings"),
        )
        .expect("legacy write");
        let upgraded = install(
            &legacy_path,
            binary(),
            true,
            true,
            false,
            policy(false, true),
        )
        .expect("upgrade");
        assert_eq!(
            upgraded.status.native_tool_mode,
            Some(NativeToolMode::Observe)
        );
        assert!(
            upgraded
                .rendered_settings
                .contains("hooks dispatch --native-mode observe")
        );

        let new_path = directory.path().join("new.json");
        let installed = install(&new_path, binary(), true, true, false, policy(false, true))
            .expect("new install");
        assert_eq!(
            installed.status.native_tool_mode,
            Some(NativeToolMode::Steer)
        );
    }

    #[test]
    fn dry_run_and_uninstall_are_recoverable() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        fs::write(&path, b"{}\n").expect("settings write");

        let preview = install(&path, binary(), true, true, false, policy(true, false))
            .expect("dry-run install");
        assert!(preview.changed);
        assert_eq!(fs::read(&path).expect("unchanged settings"), b"{}\n");

        install(&path, binary(), true, true, false, policy(false, true))
            .expect("confirmed install");
        let removed = uninstall(&path, false, true).expect("confirmed uninstall");
        assert!(removed.changed);
        assert_eq!(status(&path).expect("hook status").hzr_entries, 0);
    }

    #[test]
    fn keep_external_icm_leaves_the_duplicate_memory_layer_in_place() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        let initial = json!({
            "hooks": {
                "UserPromptSubmit": [
                    {"hooks": [{"type": "command", "command": "/home/u/.local/bin/icm hook prompt"}]}
                ]
            }
        });
        fs::write(
            &path,
            serde_json::to_vec_pretty(&initial).expect("settings JSON"),
        )
        .expect("settings write");

        install(&path, binary(), false, true, false, policy(false, true))
            .expect("install keeping external ICM");
        assert_eq!(
            status(&path).expect("hook status").external_icm_entries,
            1,
            "opting out must preserve the external ICM hook verbatim"
        );
    }

    #[test]
    fn service_opt_out_persists_in_the_session_start_hook() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        fs::write(&path, b"{}\n").expect("settings write");

        install(&path, binary(), true, false, false, policy(false, true))
            .expect("install with service opt-out");
        let opted_out = fs::read_to_string(&path).expect("opted-out settings");
        assert!(opted_out.contains("init --if-needed --quiet --session-start-hook --skip-service"));
        assert!(status(&path).expect("opted-out hook status").installed);

        install(&path, binary(), true, true, false, policy(false, true))
            .expect("restore automatic service startup");
        let automatic = fs::read_to_string(&path).expect("automatic settings");
        assert!(automatic.contains("init --if-needed --quiet"));
        assert!(
            automatic.contains("--session-start-hook"),
            "the hook must request structured user-visible update notices"
        );
        assert!(!automatic.contains("--skip-service"));
    }

    #[test]
    fn project_only_session_start_never_initializes_an_unselected_workspace() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");

        let report = install(&path, binary(), true, true, true, policy(false, true))
            .expect("project-only hook install");

        assert!(
            report
                .rendered_settings
                .contains("init --if-enabled --quiet --session-start-hook")
        );
        assert!(
            !report
                .rendered_settings
                .contains("init --if-needed --quiet")
        );
        assert!(report.status.installed);
    }

    #[test]
    fn hook_binary_refuses_build_directories_unless_explicitly_allowed() {
        let directory = tempdir().expect("temporary directory");
        let build = directory.path().join("target/debug");
        fs::create_dir_all(&build).expect("build directory");
        let executable = build.join("hzr");
        fs::write(&executable, b"#!/bin/sh\n").expect("fake binary");

        let error = resolve_hook_binary(Some(&executable), false, false)
            .expect_err("a build path must be refused");
        assert!(
            error.to_string().contains("build directory"),
            "error must explain why: {error}"
        );

        let allowed = resolve_hook_binary(Some(&executable), true, false)
            .expect("development override works");
        assert!(allowed.ends_with("hzr"));
    }

    #[test]
    fn hook_binary_rejects_a_path_that_does_not_exist() {
        assert!(
            resolve_hook_binary(Some(Path::new("/nonexistent/hzr")), true, false).is_err(),
            "a hook must never be bound to a missing executable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_binary_validates_but_preserves_a_durable_symlink() {
        let directory = tempdir().expect("temporary directory");
        let release = directory.path().join("versions/v0.4.5/bin");
        let prefix = directory.path().join("bin");
        fs::create_dir_all(&release).expect("release directory");
        fs::create_dir_all(&prefix).expect("prefix directory");
        let physical = release.join("hzr");
        fs::write(&physical, b"#!/bin/sh\n").expect("fake binary");
        let durable = prefix.join("hzr");
        std::os::unix::fs::symlink(&physical, &durable).expect("durable symlink");

        let resolved = resolve_hook_binary(Some(&durable), false, false)
            .expect("durable symlink must be accepted");
        assert_eq!(resolved, durable);
        assert!(!resolved.to_string_lossy().contains("/versions/"));
    }

    #[test]
    fn status_marks_hzr_and_rtk_coexistence_as_conflict() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        let settings = json!({
            "hooks": {
                "PreToolUse": [{"hooks": [
                    {"type": "command", "command": "'/opt/hzr' hooks dispatch"},
                    {"type": "command", "command": "/home/u/.claude/hooks/rtk-rewrite.sh"}
                ]}]
            }
        });
        fs::write(
            &path,
            serde_json::to_vec_pretty(&settings).expect("settings JSON"),
        )
        .expect("settings write");

        let current = status(&path).expect("hook status");
        assert!(current.conflict);
        assert_eq!(current.hzr_entries, 1);
        assert_eq!(current.rtk_entries, 1);
    }
}
