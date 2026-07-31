use std::ffi::OsString;
use std::path::{Path, PathBuf};
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
            version: Duration::from_secs(3),
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct InitOptions {
    pub provider: EmbeddingProvider,
    pub model: Option<String>,
    pub backend: StoreBackend,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitOutcome {
    Initialized,
    AlreadyInitialized,
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
        Ok(InitOutcome::Initialized)
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
