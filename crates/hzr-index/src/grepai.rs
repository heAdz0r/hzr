use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{IndexError, Result};
use crate::generation::IndexGeneration;
use crate::owner::IndexOwner;
use crate::process;
use crate::watch::{self, WatchHandle};
use crate::workspace::{IndexPlacement, Workspace};

pub const SUPPORTED_GREPAI_VERSION: &str = "0.35.0";
pub const SINGLE_WORKTREE_WATCH_FLAG: &str = "--no-worktree-discovery";
static CONFIG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct Deadlines {
    pub version: Duration,
    pub initialize: Duration,
    pub watch_start: Duration,
    pub watch_stop: Duration,
}

impl Default for Deadlines {
    fn default() -> Self {
        Self {
            // Process launch can exceed three seconds on a loaded CI/macOS host even
            // though the pinned binary returns immediately once scheduled.
            version: Duration::from_secs(10),
            initialize: Duration::from_secs(30),
            watch_start: Duration::from_secs(120),
            watch_stop: Duration::from_secs(35),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingProvider {
    #[default]
    Ollama,
    LmStudio,
    OpenAi,
    Synthetic,
    OpenRouter,
}

impl EmbeddingProvider {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::LmStudio => "lmstudio",
            Self::OpenAi => "openai",
            Self::Synthetic => "synthetic",
            Self::OpenRouter => "openrouter",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreBackend {
    #[default]
    Gob,
    Postgres,
    Qdrant,
}

impl StoreBackend {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Gob => "gob",
            Self::Postgres => "postgres",
            Self::Qdrant => "qdrant",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InitOptions {
    pub provider: EmbeddingProvider,
    pub model: Option<String>,
    pub backend: StoreBackend,
    pub repository_graph: bool,
}

impl Default for InitOptions {
    fn default() -> Self {
        Self {
            provider: EmbeddingProvider::default(),
            model: None,
            backend: StoreBackend::default(),
            repository_graph: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitOutcome {
    Initialized,
    AlreadyInitialized,
    RepositoryGraphEnabled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IndexStatus {
    pub placement: IndexPlacement,
    pub initialized: bool,
    pub vectors_present: bool,
    pub symbols_present: bool,
    pub repository_graph_present: bool,
    pub duplicate_index_dirs: Vec<PathBuf>,
    pub generation: Option<IndexGeneration>,
}

#[derive(Clone, Debug)]
pub struct GrepAi {
    binary: PathBuf,
    workspace: Workspace,
    deadlines: Deadlines,
    version: String,
}

impl GrepAi {
    pub async fn connect(
        binary: PathBuf,
        workspace: Workspace,
        deadlines: Deadlines,
    ) -> Result<Self> {
        let version = verify_version(&binary, &workspace.identity.root, deadlines.version).await?;
        Ok(Self {
            binary,
            workspace,
            deadlines,
            version,
        })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn status(&self) -> Result<IndexStatus> {
        let initialized = self.workspace.index.config.is_file();
        Ok(IndexStatus {
            placement: self.workspace.placement()?,
            initialized,
            vectors_present: self.workspace.index.vectors.is_file(),
            symbols_present: self.workspace.index.symbols.is_file(),
            repository_graph_present: self.workspace.index.repository_graph.is_file(),
            duplicate_index_dirs: self.workspace.duplicate_index_dirs.clone(),
            generation: initialized
                .then(|| IndexGeneration::read(&self.workspace))
                .transpose()?,
        })
    }

    pub async fn initialize(&self, options: &InitOptions) -> Result<InitOutcome> {
        self.workspace.require_single_index()?;
        self.workspace.prepare_index_location()?;
        if self.workspace.index.config.is_file() {
            if options.repository_graph && self.enable_repository_graph()? {
                return Ok(InitOutcome::RepositoryGraphEnabled);
            }
            return Ok(InitOutcome::AlreadyInitialized);
        }
        let _owner = IndexOwner::acquire(&self.workspace)?;
        if options.model.as_deref().is_some_and(str::is_empty) {
            return Err(IndexError::InvalidInput {
                field: "embedding model",
                reason: "model must not be empty".into(),
            });
        }

        let mut args = vec![
            OsString::from("init"),
            OsString::from("--yes"),
            OsString::from("--provider"),
            OsString::from(options.provider.as_str()),
            OsString::from("--backend"),
            OsString::from(options.backend.as_str()),
        ];
        if let Some(model) = &options.model {
            args.push(OsString::from("--model"));
            args.push(OsString::from(model));
        }
        let output = process::output(
            &self.binary,
            &args,
            &self.workspace.identity.root,
            self.deadlines.initialize,
            "initialize grepai",
        )
        .await?;
        process::require_success(output, "initialize grepai")?;
        self.workspace.require_initialized()?;
        if options.repository_graph {
            enable_repository_graph_in_config(&self.workspace.index.config)?;
        }
        Ok(InitOutcome::Initialized)
    }

    fn enable_repository_graph(&self) -> Result<bool> {
        let config = read_managed_config(&self.workspace.index.config)?;
        if repository_graph_enabled(&config)? {
            return Ok(false);
        }
        let _owner = IndexOwner::acquire(&self.workspace)?;
        enable_repository_graph_in_config(&self.workspace.index.config)?;
        Ok(true)
    }

    pub async fn start_watch(&self) -> Result<WatchHandle> {
        self.workspace.require_single_index()?;
        self.workspace.require_initialized()?;
        let isolated_watch = self.supports_isolated_watch().await?;
        let worktrees = self
            .workspace
            .git_worktree_count(self.deadlines.version)
            .await?;
        if worktrees > 1 && !isolated_watch {
            return Err(IndexError::UnsupportedWatchTopology {
                worktrees,
                required_flag: SINGLE_WORKTREE_WATCH_FLAG,
            });
        }
        watch::start(
            &self.binary,
            &self.workspace,
            &self.deadlines,
            isolated_watch,
        )
        .await
    }

    async fn supports_isolated_watch(&self) -> Result<bool> {
        let output = process::output(
            &self.binary,
            &[OsString::from("watch"), OsString::from("--help")],
            &self.workspace.identity.root,
            self.deadlines.version,
            "probe grepai watch capabilities",
        )
        .await?;
        let stdout = process::require_success(output, "probe grepai watch capabilities")?;
        Ok(stdout
            .windows(SINGLE_WORKTREE_WATCH_FLAG.len())
            .any(|window| window == SINGLE_WORKTREE_WATCH_FLAG.as_bytes()))
    }
}

fn enable_repository_graph_in_config(path: &Path) -> Result<()> {
    let config = read_managed_config(path)?;
    if repository_graph_enabled(&config)? {
        return Ok(());
    }

    let mut lines = config
        .split_inclusive('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if !config.ends_with('\n') && config.rsplit_once('\n').is_none() {
        lines = vec![config.clone()];
    }
    let rpg = lines.iter().position(|line| line.trim_end() == "rpg:");
    match rpg {
        Some(section) => {
            let end = lines
                .iter()
                .enumerate()
                .skip(section + 1)
                .find_map(|(index, line)| {
                    (!line.starts_with(' ') && !line.starts_with('\t') && !line.trim().is_empty())
                        .then_some(index)
                })
                .unwrap_or(lines.len());
            if let Some(enabled) = lines[section + 1..end]
                .iter()
                .position(|line| line.trim_start().starts_with("enabled:"))
                .map(|offset| section + 1 + offset)
            {
                let newline = if lines[enabled].ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                lines[enabled] = format!("    enabled: true{newline}");
            } else {
                lines.insert(section + 1, "    enabled: true\n".to_owned());
            }
        }
        None => {
            if !config.is_empty() && !config.ends_with('\n') {
                lines.push("\n".to_owned());
            }
            lines.push("rpg:\n".to_owned());
            lines.push("    enabled: true\n".to_owned());
        }
    }

    let updated = lines.concat();
    let sequence = CONFIG_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("yaml.hzr-{}-{sequence}.tmp", std::process::id()));
    let write_result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|source| IndexError::Io {
            operation: "create grepai config temporary",
            path: temporary.clone(),
            source,
        })?;
        file.write_all(updated.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|source| IndexError::Io {
                operation: "write grepai config temporary",
                path: temporary.clone(),
                source,
            })?;
        fs::rename(&temporary, path).map_err(|source| IndexError::Io {
            operation: "replace grepai config",
            path: path.to_path_buf(),
            source,
        })
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn read_managed_config(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path).map_err(|source| IndexError::Io {
        operation: "inspect grepai config",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(IndexError::InvalidInput {
            field: "grepai config",
            reason: format!("{} is not a regular file", path.display()),
        });
    }
    fs::read_to_string(path).map_err(|source| IndexError::Io {
        operation: "read grepai config",
        path: path.to_path_buf(),
        source,
    })
}

fn repository_graph_enabled(config: &str) -> Result<bool> {
    let mut in_rpg = false;
    for line in config.lines() {
        let trimmed = line.trim();
        if !(line.starts_with(' ') || line.starts_with('\t')) {
            in_rpg = trimmed == "rpg:";
            continue;
        }
        if in_rpg && trimmed.starts_with("enabled:") {
            return match trimmed.trim_start_matches("enabled:").trim() {
                "true" => Ok(true),
                "false" => Ok(false),
                value => Err(IndexError::InvalidInput {
                    field: "rpg.enabled",
                    reason: format!("expected true or false, got {value:?}"),
                }),
            };
        }
    }
    Ok(false)
}

async fn verify_version(binary: &Path, root: &Path, deadline: Duration) -> Result<String> {
    let output = process::output(
        binary,
        &[OsString::from("version")],
        root,
        deadline,
        "verify grepai version",
    )
    .await?;
    let stdout = process::require_success(output, "verify grepai version")?;
    let output = std::str::from_utf8(&stdout)
        .map_err(|error| IndexError::InvalidEngineOutput {
            engine: "grepai",
            operation: "verify version",
            detail: error.to_string(),
        })?
        .trim();
    let found = output
        .strip_prefix("grepai version ")
        .filter(|version| !version.is_empty() && !version.contains(char::is_whitespace))
        .ok_or_else(|| IndexError::InvalidEngineOutput {
            engine: "grepai",
            operation: "verify version",
            detail: format!("unexpected version output {output:?}"),
        })?;
    if found != SUPPORTED_GREPAI_VERSION {
        return Err(IndexError::UnsupportedVersion {
            expected: SUPPORTED_GREPAI_VERSION,
            found: found.to_owned(),
        });
    }
    Ok(found.to_owned())
}
