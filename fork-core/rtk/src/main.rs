mod args_utils;
mod aws_cmd; // fork: ported AWS CLI compression (upstream parity)
mod build_cmd;
mod bun_cmd;
mod cargo_cmd;
mod cc_economics;
mod ccusage;
mod config;
mod constants; // fork: shared constants (TOML filter engine)
mod container;
mod curl_cmd;
mod deps;
mod diag_summary;
mod diff_cmd;
mod discover;
mod display_helpers;
mod env_cmd;
mod fidelity;
mod filter;
mod find_cmd;
mod format_cmd;
mod gain;
mod gh_cmd;
mod git;
mod go_cmd;
mod golangci_cmd;
mod grepai;
mod gt_cmd; // fork: Graphite stacked-PR compact output
mod guard;
mod hook_audit_cmd; // upstream sync: hook rewrite audit metrics // grepai external semantic search integration
mod init;
mod integrity; // fork: hook integrity verification (SHA-256)
mod json_cmd;
mod jsonpack; // lossless CSV+schema repacking for gh --json / gh api
mod learn;
mod lint_cmd;
mod local_llm;
mod log_cmd;
mod ls;
mod lsof_cmd;
mod memory_layer;
mod mypy_cmd; // fork: mypy grouped error output
mod next_cmd;
mod npm_cmd;
mod parser;
mod permissions; // fork: Claude Code permission rules for rewrite verdicts
mod pip_cmd;
mod pipe_cmd; // fork: stdin pipe filtering (upstream v0.42.4)
mod playwright_cmd;
mod pnpm_cmd;
mod prettier_cmd;
mod prisma_cmd;
mod ps_cmd;
mod psql_cmd; // fork: psql table/expanded output compression
mod pytest_cmd;
mod read;
mod read_cache; // PR-2: extracted read cache logic
mod read_changed; // PR-5: git-aware diff reading
mod read_digest; // PR-2: extracted tabular digest logic
mod read_render; // PR-2: extracted render helpers
mod read_source; // PR-2: extracted source I/O and line-range logic
mod read_symbols; // PR-3: symbol model and extraction traits
mod read_types; // PR-2: shared read types (ReadMode, ReadRequest)
mod remote_logs_cmd;
mod rewrite_cmd; // fork: single source of truth for hook rewrites
mod rgai_cmd; // semantic search command (grepai-style intent matching)
mod ruff_cmd;
mod runner;
mod search;
mod session_stats; // cache compounding savings calculator
mod sqlite_cmd;
mod ssh_cmd;
mod stream;
mod summary;
mod symbols_regex; // PR-3: regex-based symbol extractor
mod tar_cmd;
mod tee; // upstream sync: tee raw output to file for LLM re-read
mod toml_filter; // fork: declarative TOML filter engine (upstream v0.42.4)
mod tracking;
mod tree;
mod truncate; // upstream v0.41: global caps for filter output limits
mod trust; // fork: trust-gating for project-local TOML filters
mod tsc_cmd;
mod utils;
mod verify_cmd; // fork: TOML filter inline tests
mod vitest_cmd;
mod wc_cmd;
mod wget_cmd;
mod write_cmd;
mod write_core;
mod write_lock; // changed: flock abstraction for concurrent write safety
mod write_semantics;

use anyhow::{Context, Result};
use clap::error::ErrorKind; // fix #200
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const CAPABILITY_BATCH_MAX_BYTES: u64 = 8 * 1024 * 1024;
const CAPABILITY_BATCH_MAX_COMMANDS: usize = 100_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityBatchRequest {
    commands: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CapabilityBatchResponse {
    supported: Vec<bool>,
}

#[derive(Parser)]
#[command(
    name = "rtk",
    version = env!("CARGO_PKG_VERSION"),
    about = "Rust Token Killer - Minimize LLM token consumption",
    long_about = "A high-performance CLI proxy designed to filter and summarize system outputs before they reach your LLM context."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Verbosity level (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Ultra-compact mode: ASCII icons, inline format (Level 2 optimizations)
    #[arg(short = 'u', long, global = true)]
    ultra_compact: bool,

    /// Set SKIP_ENV_VALIDATION=1 for child processes (Next.js, tsc, lint, prisma)
    #[arg(long = "skip-env", global = true)]
    skip_env: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Internal typed capability probe used by the HZR stats collector.
    #[command(name = "capability-batch", hide = true)]
    CapabilityBatch,

    /// List directory contents with token-optimized output (proxy to native ls)
    Ls {
        /// Arguments passed to ls (supports all native ls flags like -l, -a, -h, -R)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Directory tree with token-optimized output (proxy to native tree)
    Tree {
        /// Arguments passed to tree (supports all native tree flags like -L, -d, -a)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Read a file with intelligent filtering and format-aware digests
    Read {
        /// File to read
        file: PathBuf,
        /// Additional files for --batch, kept in caller order
        #[arg(value_name = "FILE")]
        additional_files: Vec<PathBuf>,
        /// Read several files under one total output budget
        #[arg(long, requires = "max_tokens")]
        batch: bool,
        /// Maximum estimated output tokens across a batch
        #[arg(long, requires = "batch")]
        max_tokens: Option<usize>,
        /// Maximum estimated output tokens for each file in a batch
        #[arg(long, requires = "batch")]
        per_file_tokens: Option<usize>,
        /// Filter level: none (exact), minimal (strips blanks/comments), aggressive.
        /// Default: auto — none if --from/--to given (edit mode), minimal for code, none for config/data. // changed: smart default level
        #[arg(short, long)]
        level: Option<filter::FilterLevel>,
        /// Start line (1-based, inclusive)
        #[arg(long)]
        from: Option<usize>,
        /// End line (1-based, inclusive)
        #[arg(long)]
        to: Option<usize>,
        /// Keep the exact first N lines (head semantics)
        #[arg(short, long, conflicts_with = "tail_lines")]
        max_lines: Option<usize>,
        /// Keep only last N lines (tail semantics) // fork: ported from upstream v0.42.4
        #[arg(long)]
        tail_lines: Option<usize>,
        /// Show source line numbers (defaults to exact content)
        #[arg(short = 'n', long)]
        line_numbers: bool,

        // ── Read mode flags (mutually exclusive) ──
        /// Show Markdown headings or supported code symbols with line spans
        #[arg(long, group = "read_mode")]
        outline: bool,
        /// Show machine-readable JSON symbol index
        #[arg(long, group = "read_mode")]
        symbols: bool,
        /// Show only changed hunks from git working tree
        #[arg(long, group = "read_mode")]
        changed: bool,
        /// Show changed hunks relative to a revision (e.g., HEAD~3, main)
        #[arg(long, group = "read_mode")]
        since: Option<String>,
        /// Context lines for --changed/--since (default: 3)
        #[arg(long, default_value = "3", requires = "read_mode")]
        diff_context: usize,
        /// Deduplicate repetitive blocks (opt-in, disabled by default)
        #[arg(long)]
        dedup: bool,
    },

    /// Generate 2-line technical summary (heuristic-based)
    Smart {
        /// File to analyze
        file: PathBuf,
        /// Model: heuristic
        #[arg(short, long, default_value = "heuristic")]
        model: String,
        /// Force model download
        #[arg(long)]
        force_download: bool,
    },

    /// Git commands with compact output
    Git {
        /// Change to directory before executing (like git -C <path>, can be repeated)
        #[arg(short = 'C', action = clap::ArgAction::Append)]
        directory: Vec<String>,

        /// Git configuration override (like git -c key=value, can be repeated)
        #[arg(short = 'c', action = clap::ArgAction::Append)]
        config_override: Vec<String>,

        /// Set the path to the .git directory
        #[arg(long = "git-dir")]
        git_dir: Option<String>,

        /// Set the path to the working tree
        #[arg(long = "work-tree")]
        work_tree: Option<String>,

        /// Disable pager (like git --no-pager)
        #[arg(long = "no-pager")]
        no_pager: bool,

        /// Skip optional locks (like git --no-optional-locks)
        #[arg(long = "no-optional-locks")]
        no_optional_locks: bool,

        /// Treat repository as bare (like git --bare)
        #[arg(long)]
        bare: bool,

        /// Treat pathspecs literally (like git --literal-pathspecs)
        #[arg(long = "literal-pathspecs")]
        literal_pathspecs: bool,

        #[command(subcommand)]
        command: GitCommands,
    },

    /// Safe file writes (atomic + idempotent) with dry-run support
    Write {
        /// Output mode: quiet (silent on success), concise (default), json (machine-readable)
        #[arg(long, value_enum, default_value = "concise")]
        output: write_cmd::OutputMode,
        #[command(subcommand)]
        command: WriteCommands,
    },

    /// Build/install/verify rtk binaries (native replacement for rtk-build.sh)
    Build {
        #[command(subcommand)]
        command: BuildCommands,
    },

    /// GitHub CLI (gh) commands with token-optimized output
    Gh {
        /// Subcommand: pr, issue, run, repo
        subcommand: String,
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// pnpm commands with ultra-compact output
    Pnpm {
        #[command(subcommand)]
        command: PnpmCommands,
    },

    /// Run command and show only errors/warnings
    Err {
        /// Command to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    /// Run tests and show only failures
    Test {
        /// Test command (e.g. cargo test)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    /// Show JSON structure without values
    Json {
        /// JSON file
        file: PathBuf,
        /// Max depth
        #[arg(short, long, default_value = "5")]
        depth: usize,
    },

    /// Summarize project dependencies
    Deps {
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Show environment variables (filtered, sensitive masked)
    Env {
        /// Filter by name (e.g. PATH, AWS)
        #[arg(short, long)]
        filter: Option<String>,
        /// Show all (include sensitive)
        #[arg(long)]
        show_all: bool,
    },

    /// Find files with compact tree output (supports native find flags: -name, -type, -maxdepth, -iname)
    #[command(trailing_var_arg = true, allow_hyphen_values = true)] // fix #211
    Find {
        /// Args: native (-name pattern -type f -maxdepth N) or RTK (pattern [path] [-m N] [-t f|d])
        args: Vec<String>,
    },

    /// Ultra-condensed diff (only changed lines)
    Diff {
        /// First file or - for stdin (unified diff)
        file1: PathBuf,
        /// Second file (optional if stdin)
        file2: Option<PathBuf>,
    },

    /// Filter and deduplicate log output
    Log {
        /// Log file (omit for stdin)
        file: Option<PathBuf>,
    },

    /// Docker commands with compact output
    Docker {
        #[command(subcommand)]
        command: DockerCommands,
    },

    /// Kubectl commands with compact output
    Kubectl {
        #[command(subcommand)]
        command: KubectlCommands,
    },

    /// OpenShift CLI commands with compact output
    Oc {
        #[command(subcommand)]
        command: OcCommands,
    },

    /// Run command and show heuristic summary
    Summary {
        /// Command to run and summarize
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    /// Compact grep - strips whitespace, truncates, groups by file
    Grep {
        /// Max line length
        #[arg(short = 'l', long, default_value = "80")]
        max_len: usize,
        /// Max results to show
        #[arg(short, long, default_value = "200")]
        max: usize,
        /// Show only match context (not full line)
        #[arg(long)]
        context_only: bool,
        /// Filter by file type (e.g., ts, py, rust)
        #[arg(short = 't', long)]
        file_type: Option<String>,
        /// Pattern, path, and any grep/rg flags (e.g. -v, -i, -A 3, --glob, --version)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra_args: Vec<String>,
    },

    /// Compact ripgrep - runs rg natively, same output filter as grep
    Rg {
        /// Pattern, path, and any rg flags (e.g. -v, -i, -t rust, --glob)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra_args: Vec<String>,
    },

    /// Rust-native semantic search (grepai-style intent matching)
    Rgai {
        /// Natural-language query
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// Path to search in
        #[arg(short, long, default_value = ".")]
        path: String,
        /// Max files to show
        #[arg(short, long, default_value = "8")]
        max: usize,
        /// Context lines around each match
        #[arg(short = 'c', long, default_value = "1")]
        context: usize,
        /// Filter by file type (e.g., ts, py, rust)
        #[arg(short = 't', long)]
        file_type: Option<String>,
        /// Skip files larger than N KB
        #[arg(long, default_value = "512")]
        max_file_kb: usize,
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Compact output (fewer lines per hit)
        #[arg(long)]
        compact: bool,
        /// Force built-in keyword search (skip grepai delegation)
        #[arg(long)]
        builtin: bool,
        /// Match the query verbatim and case-sensitively instead of ranking its terms
        ///
        /// fork: without this the query is lowercased, split on non-alphanumerics,
        /// stripped of stop words, stemmed and OR-ed, so `rtk rgai "fn handle_request"`
        /// matches every `fn` in the tree. Implies --builtin.
        #[arg(long)]
        literal: bool,
        /// Restrict search to specific files (comma-separated paths)
        #[arg(long)]
        files: Option<String>, // ADDED: --files flag for two-stage memory pipeline
        /// Project root that owns the grepai index; --path remains a search filter
        #[arg(long, hide = true)]
        project_root: Option<String>,
    },

    /// Initialize rtk instructions for assistant CLI usage
    Init {
        /// Add to global ~/.claude/CLAUDE.md instead of local
        #[arg(short, long)]
        global: bool,

        /// Show current configuration
        #[arg(long)]
        show: bool,

        /// Inject full instructions into CLAUDE.md (legacy mode)
        #[arg(long = "claude-md", group = "mode")]
        claude_md: bool,

        /// Hook only, no RTK.md
        #[arg(long = "hook-only", group = "mode")]
        hook_only: bool,

        /// Auto-patch settings.json without prompting
        #[arg(long = "auto-patch", group = "patch")]
        auto_patch: bool,

        /// Skip settings.json patching (print manual instructions)
        #[arg(long = "no-patch", group = "patch")]
        no_patch: bool,

        /// Remove all RTK artifacts (hook, RTK.md, CLAUDE.md reference, settings.json entry)
        #[arg(long)]
        uninstall: bool,

        /// Target Codex CLI (uses AGENTS.md + RTK.md)
        #[arg(long)]
        codex: bool,

        /// Trust and enable detected custom filters without prompting
        #[arg(long = "trust-filters", group = "trust")]
        trust_filters: bool,

        /// Leave detected custom filters disabled without prompting
        #[arg(long = "no-trust-filters", group = "trust")]
        no_trust_filters: bool,

        /// Preview changes without writing any files
        #[arg(long = "dry-run", conflicts_with = "show")]
        dry_run: bool,
    },

    /// Download with compact output (strips progress bars)
    Wget {
        /// URL to download
        url: String,
        /// Output to stdout instead of file
        #[arg(short = 'O', long)]
        stdout: bool,
        /// Additional wget arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Word/line/byte count with compact output (strips paths and padding)
    Wc {
        /// Arguments passed to wc (files, flags like -l, -w, -c)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Show token savings summary and history
    Gain {
        /// Filter statistics to current project (current working directory) // added
        #[arg(short, long)]
        project: bool,
        /// Show ASCII graph of daily savings
        #[arg(short, long)]
        graph: bool,
        /// Show recent command history
        #[arg(short = 'H', long)]
        history: bool,
        /// Show monthly quota savings estimate
        #[arg(short, long)]
        quota: bool,
        /// Subscription tier for quota calculation: pro, 5x, 20x
        #[arg(short, long, default_value = "20x", requires = "quota")]
        tier: String,
        /// Show detailed daily breakdown (all days)
        #[arg(short, long)]
        daily: bool,
        /// Show weekly breakdown
        #[arg(short, long)]
        weekly: bool,
        /// Show monthly breakdown
        #[arg(short, long)]
        monthly: bool,
        /// Show all time breakdowns (daily + weekly + monthly)
        #[arg(short, long)]
        all: bool,
        /// Output format: text, json, csv
        #[arg(short, long, default_value = "text")]
        format: String,
        /// Show parse failure log (commands that fell back to raw execution)
        #[arg(short = 'F', long)] // fix #200
        failures: bool,
    },

    /// Claude Code economics: spending (ccusage) vs savings (rtk) analysis
    CcEconomics {
        /// Show detailed daily breakdown
        #[arg(short, long)]
        daily: bool,
        /// Show weekly breakdown
        #[arg(short, long)]
        weekly: bool,
        /// Show monthly breakdown
        #[arg(short, long)]
        monthly: bool,
        /// Show all time breakdowns (daily + weekly + monthly)
        #[arg(short, long)]
        all: bool,
        /// Output format: text, json, csv
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Show or create configuration file
    Config {
        /// Create default config file
        #[arg(long)]
        create: bool,
        /// Output format: text or json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },

    /// Vitest commands with compact output
    Vitest {
        #[command(subcommand)]
        command: VitestCommands,
    },

    /// Prisma commands with compact output (no ASCII art)
    Prisma {
        #[command(subcommand)]
        command: PrismaCommands,
    },

    /// TypeScript compiler with grouped error output
    Tsc {
        /// TypeScript compiler arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Next.js build with compact output
    Next {
        /// Next.js build arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// ESLint with grouped rule violations
    Lint {
        /// Linter arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Prettier format checker with compact output
    Prettier {
        /// Prettier arguments (e.g., --check, --write)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Universal format checker (prettier, black, ruff format)
    Format {
        /// Formatter arguments (auto-detects formatter from project files)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Playwright E2E tests with compact output
    Playwright {
        /// Playwright arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Cargo commands with compact output
    Cargo {
        #[command(subcommand)]
        command: CargoCommands,
    },

    /// npm run with filtered output (strip boilerplate)
    Npm {
        /// npm run arguments (script name + options)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// npx with intelligent routing (tsc, eslint, prisma -> specialized filters)
    Npx {
        /// npx arguments (command + options)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Bun commands with compact output (script boilerplate stripped)
    Bun {
        /// Bun arguments (e.g., run typecheck, test, --version)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Curl with auto-JSON detection and schema output
    Curl {
        /// Curl arguments (URL + options)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// SSH with smart output filtering (psql/json/html/generic)
    Ssh {
        /// SSH arguments (host + remote command + flags)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// lsof with compact port/socket output (groups by port, shows LISTEN + connection counts)
    Lsof {
        /// lsof arguments (e.g. -i :8080 -i :3000)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// ps with compact user-process table (filters system procs, sorts by CPU)
    Ps {
        /// ps arguments (e.g. aux, -p PID)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Read-only SQLite SELECT with bounded rows, columns, and output
    Sqlite3 {
        /// SQLite database path
        database: PathBuf,
        /// One SELECT statement
        query: String,
        /// Project these result columns (comma-separated or repeated)
        #[arg(long, value_delimiter = ',')]
        columns: Vec<String>,
        /// Maximum rows (1-500); one extra row is probed for recovery reporting
        #[arg(long, default_value_t = 50)]
        max_rows: usize,
        /// Maximum estimated output tokens (64-8192)
        #[arg(long, default_value_t = 2_048)]
        max_tokens: usize,
    },

    /// List archive members with a bounded entry count
    Tar {
        /// Maximum listed entries (1-1000)
        #[arg(long, default_value_t = tar_cmd::default_max_entries())]
        max_entries: usize,
        /// Native tar listing arguments (-tf/-tzf/--list only)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        args: Vec<String>,
    },

    /// Fetch a bounded tail from Docker logs over an SSH host alias
    Logs {
        /// SSH destination or configured host alias
        host: String,
        /// Docker container name or ID
        container: String,
        /// Remote docker log tail (1-1000)
        #[arg(long, default_value_t = 100)]
        tail: usize,
        /// Docker --since duration or timestamp
        #[arg(long)]
        since: Option<String>,
        /// Include Docker timestamps
        #[arg(long)]
        timestamps: bool,
    },

    /// Discover missed RTK savings from Claude Code history
    Discover {
        /// Filter by project path (substring match)
        #[arg(short, long)]
        project: Option<String>,
        /// Max commands per section
        #[arg(short, long, default_value = "15")]
        limit: usize,
        /// Scan all projects (default: current project only)
        #[arg(short, long)]
        all: bool,
        /// Limit to sessions from last N days
        #[arg(short, long, default_value = "30")]
        since: u64,
        /// Output format: text, json
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Shared project memory, cache artifacts, and incremental deltas
    Memory {
        #[command(subcommand)]
        command: MemoryCommands,
    },

    /// Learn CLI corrections from Claude Code error history
    Learn {
        /// Filter by project path (substring match)
        #[arg(short, long)]
        project: Option<String>,
        /// Scan all projects (default: current project only)
        #[arg(short, long)]
        all: bool,
        /// Limit to sessions from last N days
        #[arg(short, long, default_value = "30")]
        since: u64,
        /// Output format: text, json
        #[arg(short, long, default_value = "text")]
        format: String,
        /// Generate .claude/rules/cli-corrections.md file
        #[arg(short, long)]
        write_rules: bool,
        /// Minimum confidence threshold (0.0-1.0)
        #[arg(long, default_value = "0.6")]
        min_confidence: f64,
        /// Minimum occurrences to include in report
        #[arg(long, default_value = "1")]
        min_occurrences: usize,
    },

    /// Execute command without filtering but track usage
    #[command(visible_alias = "raw")]
    Proxy {
        /// Command and arguments to execute
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },

    /// Ruff linter/formatter with compact output
    Ruff {
        /// Ruff arguments (e.g., check, format --check)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Pytest test runner with compact output
    Pytest {
        /// Pytest arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Pip package manager with compact output (auto-detects uv)
    Pip {
        /// Pip arguments (e.g., list, outdated, install)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Go commands with compact output
    Go {
        #[command(subcommand)]
        command: GoCommands,
    },

    /// golangci-lint with compact output
    #[command(name = "golangci-lint")]
    GolangciLint {
        /// golangci-lint arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Show hook rewrite audit metrics (requires RTK_HOOK_AUDIT=1) // upstream sync
    #[command(name = "hook-audit")]
    HookAudit {
        /// Show entries from last N days (0 = all time)
        #[arg(short, long, default_value = "7")]
        since: u64,
    },

    /// AWS CLI with compact output (force JSON, compress)
    Aws {
        /// AWS service subcommand (e.g., sts, s3, ec2, ecs, rds, cloudformation)
        subcommand: String,
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// PostgreSQL client with compact output (strip borders, compress tables)
    #[command(disable_help_flag = true)]
    Psql {
        /// psql arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Mypy type checker with grouped error output
    Mypy {
        /// Mypy arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Graphite (gt) stacked PR commands with compact output
    Gt {
        #[command(subcommand)]
        command: GtCommands,
    },

    /// Read stdin, apply filter, print filtered output (Unix pipe mode)
    Pipe {
        /// Filter name (cargo-test, pytest, grep, find, git-log, etc.)
        #[arg(short, long)]
        filter: Option<String>,

        /// Pass stdin through without filtering
        #[arg(long)]
        passthrough: bool,
    },

    /// Trust project-local TOML filters in current directory
    Trust {
        /// List all trusted projects
        #[arg(long)]
        list: bool,
    },

    /// Revoke trust for project-local TOML filters
    Untrust,

    /// Verify hook integrity and run TOML filter inline tests
    Verify {
        /// Run tests only for this filter name
        #[arg(long)]
        filter: Option<String>,
        /// Fail if any filter has no inline tests (CI mode)
        #[arg(long)]
        require_all: bool,
    },

    /// Rewrite a raw command to its RTK equivalent (single source of truth for hooks)
    ///
    /// Exits 0 and prints the rewritten command if supported.
    /// Exits 1 with no output if the command has no RTK equivalent.
    ///
    /// Used by Claude Code, Gemini CLI, and other LLM hooks:
    ///   REWRITTEN=$(rtk rewrite "$CMD") || exit 0
    Rewrite {
        /// Raw command to rewrite (e.g. "git status", "cargo test && git push")
        /// Accepts multiple args: `rtk rewrite ls -al` is equivalent to `rtk rewrite "ls -al"`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Emit one typed JSON rewrite decision for managed callers
    #[command(name = "rewrite-plan", hide = true)]
    RewritePlan {
        /// Raw command to classify and rewrite
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum GtCommands {
    /// Compact stack log output
    Log {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact submit output
    Submit {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact sync output
    Sync {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact restack output
    Restack {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact create output
    Create {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Branch info and management
    Branch {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: git-passthrough detection or direct gt execution
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum GitCommands {
    /// Condensed diff output
    Diff {
        /// Git arguments (supports all git diff flags like --stat, --cached, etc)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// One-line commit history
    Log {
        /// Git arguments (supports all git log flags like --oneline, --graph, --all)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact status (supports all git status flags)
    Status {
        /// Git arguments (supports all git status flags like --porcelain, --short, -s)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact show (commit summary + stat + compacted diff)
    Show {
        /// Git arguments (supports all git show flags)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Add files → "ok ✓"
    Add {
        /// Files and flags to add (supports all git add flags like -A, -p, --all, etc)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Commit → "ok ✓ \<hash\>"
    Commit {
        /// Commit message
        #[arg(short, long)]
        message: String,
    },
    /// Compact checkout result
    Checkout {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Push → "ok ✓ \<branch\>"
    Push {
        /// Git push arguments (supports -u, remote, branch, etc.)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Pull → "ok ✓ \<stats\>"
    Pull {
        /// Git pull arguments (supports --rebase, remote, branch, etc.)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact branch listing (current/local/remote)
    Branch {
        /// Git branch arguments (supports -d, -D, -m, etc.)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Fetch → "ok fetched (N new refs)"
    Fetch {
        /// Git fetch arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Stash management (list, show, pop, apply, drop)
    Stash {
        /// Subcommand: list, show, pop, apply, drop, push
        subcommand: Option<String>,
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact worktree listing
    Worktree {
        /// Git worktree arguments (add, remove, prune, or empty for list)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Grouped blame ranges with commit, author, date, and summary
    Blame {
        /// Git blame arguments, including -L ranges and pathspecs
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: runs any unsupported git subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum WriteCommands {
    /// Replace exact text in a file (first match by default)
    Replace {
        /// Target file
        file: PathBuf,
        /// Text to find (exact match). Use @/path/file to read from a file (avoids single-quote shell conflicts).
        #[arg(long, allow_hyphen_values = true)]
        from: String,
        /// Replacement text. Use @/path/file to read from a file (avoids single-quote shell conflicts).
        #[arg(long, allow_hyphen_values = true)]
        to: String,
        /// Replace all matches
        #[arg(long)]
        all: bool,
        /// Preview without writing
        #[arg(long)]
        dry_run: bool,
        /// Use fast durability mode (skip fsync)
        #[arg(long)]
        fast: bool,
        /// Enable explicit CAS (compare-and-swap) check // changed: concurrency flag
        #[arg(long)]
        cas: bool,
        /// Retry on conflict (auto-enables CAS when > 0) // changed: concurrency flag
        #[arg(long, default_value = "0")]
        retry: u32,
    },
    /// Apply exact old->new hunk replacement
    Patch {
        /// Target file
        file: PathBuf,
        /// Old block to replace (exact match). Use @/path/file to read from a file (avoids single-quote shell conflicts).
        #[arg(long, allow_hyphen_values = true)]
        old: String,
        /// New block. Use @/path/file to read from a file (avoids single-quote shell conflicts).
        #[arg(long = "new", allow_hyphen_values = true)]
        new_text: String,
        /// Replace all matching hunks
        #[arg(long)]
        all: bool,
        /// Preview without writing
        #[arg(long)]
        dry_run: bool,
        /// Use fast durability mode (skip fsync)
        #[arg(long)]
        fast: bool,
        /// Enable explicit CAS (compare-and-swap) check // changed: concurrency flag
        #[arg(long)]
        cas: bool,
        /// Retry on conflict (auto-enables CAS when > 0) // changed: concurrency flag
        #[arg(long, default_value = "0")]
        retry: u32,
    },
    /// Set structured config key in JSON/TOML file
    Set {
        /// Target file
        file: PathBuf,
        /// Dotted key path (e.g. hooks.PreToolUse.0.matcher)
        #[arg(long)]
        key: String,
        /// Value payload
        #[arg(long)]
        value: String,
        /// Value parser
        #[arg(long, value_enum, default_value = "auto")]
        value_type: write_cmd::ConfigValueType,
        /// Config format
        #[arg(long, value_enum, default_value = "auto")]
        format: write_cmd::ConfigFormat,
        /// Preview without writing
        #[arg(long)]
        dry_run: bool,
        /// Use fast durability mode (skip fsync)
        #[arg(long)]
        fast: bool,
        /// Enable explicit CAS (compare-and-swap) check // changed: concurrency flag
        #[arg(long)]
        cas: bool,
        /// Retry on conflict (auto-enables CAS when > 0) // changed: concurrency flag
        #[arg(long, default_value = "0")]
        retry: u32,
    },
    /// Execute a batch of write operations from a JSON plan (single process, grouped I/O)
    Batch {
        /// JSON plan: array of ops [{op:"replace",file:"...",from:"...",to:"..."}, ...]
        #[arg(long)]
        plan: String,
        /// Preview without writing
        #[arg(long)]
        dry_run: bool,
        /// Use fast durability mode (skip fsync)
        #[arg(long)]
        fast: bool,
        /// Enable explicit CAS (compare-and-swap) check // changed: concurrency flag
        #[arg(long)]
        cas: bool,
        /// Retry on conflict (auto-enables CAS when > 0) // changed: concurrency flag
        #[arg(long, default_value = "0")]
        retry: u32,
    },
    /// Create a new file with the given content (atomic, idempotent). Also available as `file` alias. // changed: add "file" alias
    #[command(alias = "file")] // changed: rtk write file <path> --content @/tmp/f
    Create {
        /// Target file path (created including parent directories)
        file: PathBuf,
        /// File content. Use @path to read from file, @- for stdin.
        #[arg(long, default_value = "")]
        content: String,
        /// Overwrite existing file with different content // changed: fork.2 temp-file workflow
        #[arg(long)]
        force: bool,
        /// Preview without writing
        #[arg(long)]
        dry_run: bool,
        /// Use fast durability mode (skip fsync)
        #[arg(long)]
        fast: bool,
    },
}

#[derive(Subcommand)]
enum BuildCommands {
    /// Full build pipeline compatible with legacy rtk-build.sh options
    Sh {
        /// Project root containing Cargo.toml
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Skip `cargo build`
        #[arg(long)]
        no_debug: bool,
        /// Skip `cargo build --release`
        #[arg(long)]
        no_release: bool,
        /// Skip install to ~/.cargo/bin/rtk
        #[arg(long)]
        skip_user: bool,
        /// Skip install to /usr/local/bin/rtk
        #[arg(long)]
        skip_usr_local: bool,
        /// Set /usr/local/bin/rtk -> ~/.cargo/bin/rtk symlink
        #[arg(long)]
        symlink_usr_local: bool,
        /// Update package version in Cargo.toml before build
        #[arg(long)]
        set_version: Option<String>,
        /// Skip post-build verification
        #[arg(long)]
        no_verify: bool,
        /// Never invoke sudo
        #[arg(long)]
        no_sudo: bool,
        /// Skip automatic global `rtk init -g --auto-patch` sync after build/install
        #[arg(long)]
        no_init_sync: bool,
    },
}

#[derive(Subcommand)]
enum PnpmCommands {
    /// List installed packages (ultra-dense)
    List {
        /// Depth level (default: 0)
        #[arg(short, long, default_value = "0")]
        depth: usize,
        /// Additional pnpm arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Show outdated packages (condensed: "pkg: old → new")
    Outdated {
        /// Additional pnpm arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Install packages (filter progress bars)
    Install {
        /// Packages to install
        packages: Vec<String>,
        /// Additional pnpm arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Build (delegates to next build filter)
    Build {
        /// Additional build arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Typecheck (delegates to tsc filter)
    Typecheck {
        /// Additional typecheck arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: runs any unsupported pnpm subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum DockerCommands {
    /// List running containers
    Ps,
    /// List images
    Images,
    /// Show container logs (deduplicated)
    Logs { container: String },
    /// Docker Compose commands with compact output
    Compose {
        #[command(subcommand)]
        command: ComposeCommands,
    },
    /// Passthrough: runs any unsupported docker subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

// upstream sync 0.21.0: docker compose support
#[derive(Subcommand)]
enum ComposeCommands {
    /// List compose services (compact)
    Ps,
    /// Show compose logs (deduplicated)
    Logs {
        /// Optional service name
        service: Option<String>,
    },
    /// Build compose services (summary)
    Build {
        /// Optional service name
        service: Option<String>,
    },
    /// Passthrough: runs any unsupported compose subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum KubectlCommands {
    /// Get Kubernetes resources (compact for pods/services)
    Get {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List pods
    Pods {
        #[arg(short, long)]
        namespace: Option<String>,
        /// All namespaces
        #[arg(short = 'A', long)]
        all: bool,
    },
    /// List services
    Services {
        #[arg(short, long)]
        namespace: Option<String>,
        /// All namespaces
        #[arg(short = 'A', long)]
        all: bool,
    },
    /// Show pod logs (deduplicated)
    Logs {
        pod: String,
        #[arg(short, long)]
        container: Option<String>,
    },
    /// Passthrough: runs any unsupported kubectl subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum OcCommands {
    /// Get OpenShift resources (compact for pods/services)
    Get {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List pods
    Pods {
        #[arg(short, long)]
        namespace: Option<String>,
        #[arg(short = 'A', long)]
        all: bool,
    },
    /// List services
    Services {
        #[arg(short, long)]
        namespace: Option<String>,
        #[arg(short = 'A', long)]
        all: bool,
    },
    /// Show pod logs (deduplicated)
    Logs {
        pod: String,
        #[arg(short, long)]
        container: Option<String>,
    },
    /// Passthrough: runs any unsupported oc subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum VitestCommands {
    /// Run tests with filtered output (90% token reduction)
    Run {
        /// Additional vitest arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum PrismaCommands {
    /// Generate Prisma Client (strip ASCII art)
    Generate {
        /// Additional prisma arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Manage migrations
    Migrate {
        #[command(subcommand)]
        command: PrismaMigrateCommands,
    },
    /// Push schema to database
    DbPush {
        /// Additional prisma arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum PrismaMigrateCommands {
    /// Create and apply migration
    Dev {
        /// Migration name
        #[arg(short, long)]
        name: Option<String>,
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Check migration status
    Status {
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Deploy migrations to production
    Deploy {
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum CargoCommands {
    /// Build with compact output (strip Compiling lines, keep errors)
    Build {
        /// Additional cargo build arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Test with failures-only output
    Test {
        /// Additional cargo test arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Clippy with warnings grouped by lint rule
    Clippy {
        /// Additional cargo clippy arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Check with compact output (strip Checking lines, keep errors)
    Check {
        /// Additional cargo check arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Install with compact output (strip dep compilation, keep installed/errors)
    Install {
        /// Additional cargo install arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Nextest with failures-only output
    Nextest {
        /// Additional cargo nextest arguments (e.g., run, list, --lib)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: runs any unsupported cargo subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum GoCommands {
    /// Run tests with compact output (90% token reduction via JSON streaming)
    Test {
        /// Additional go test arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Build with compact output (errors only)
    Build {
        /// Additional go build arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Vet with compact output
    Vet {
        /// Additional go vet arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Run a Go program with compact build-error output
    Run {
        /// Additional go run arguments (package path and flags)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: runs any unsupported go subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum MemoryCommands {
    /// Build/reuse project index and return context slice
    Explore {
        /// Project root to index
        #[arg(default_value = ".")]
        project: PathBuf,
        /// Force full rehash and rebuild
        #[arg(long)]
        refresh: bool,
        /// P1: Strict dirty-blocking: exit with error if artifact is STALE or DIRTY (no auto-rebuild)
        #[arg(long)]
        strict: bool,
        /// Response detail level
        #[arg(long, value_enum, default_value = "compact")]
        detail: memory_layer::DetailLevel,
        /// Output format: text, json
        #[arg(short, long, default_value = "text")]
        format: String,
        /// Relevance filter: general, bugfix, feature, refactor, incident
        #[arg(long, value_enum, default_value = "general")]
        query_type: memory_layer::QueryType, // E2.3
    },
    /// Return only changed files/modules since last artifact
    Delta {
        /// Project root to inspect
        #[arg(default_value = ".")]
        project: PathBuf,
        /// Git base revision for delta (e.g. HEAD~5, origin/main)
        #[arg(long)]
        since: Option<String>,
        /// Response detail level
        #[arg(long, value_enum, default_value = "compact")]
        detail: memory_layer::DetailLevel,
        /// Output format: text, json
        #[arg(short, long, default_value = "text")]
        format: String,
        /// Relevance filter: general, bugfix, feature, refactor, incident
        #[arg(long, value_enum, default_value = "general")]
        query_type: memory_layer::QueryType, // E2.3
    },
    /// Force artifact rebuild and persist fresh snapshot
    Refresh {
        /// Project root to refresh
        #[arg(default_value = ".")]
        project: PathBuf,
        /// Response detail level
        #[arg(long, value_enum, default_value = "compact")]
        detail: memory_layer::DetailLevel,
        /// Output format: text, json
        #[arg(short, long, default_value = "text")]
        format: String,
        /// Relevance filter: general, bugfix, feature, refactor, incident
        #[arg(long, value_enum, default_value = "general")]
        query_type: memory_layer::QueryType, // E2.3
    },
    /// Poll project and emit deltas continuously
    Watch {
        /// Project root to watch
        #[arg(default_value = ".")]
        project: PathBuf,
        /// Debounce window in seconds (E3.1: event-driven via notify)
        #[arg(long, default_value = "2")]
        interval: u64,
        /// Response detail level
        #[arg(long, value_enum, default_value = "compact")]
        detail: memory_layer::DetailLevel,
        /// Output format: text, json
        #[arg(short, long, default_value = "text")]
        format: String,
        /// Relevance filter: general, bugfix, feature, refactor, incident
        #[arg(long, value_enum, default_value = "general")]
        query_type: memory_layer::QueryType, // E2.3
    },

    /// Show cache status (FRESH/STALE/DIRTY, files, bytes, age)
    Status {
        /// Project root to inspect (default: current directory)
        #[arg(default_value = ".")]
        project: PathBuf,
    },

    /// Remove cached artifacts for this project
    Clear {
        /// Project root to clear (default: current directory)
        #[arg(default_value = ".")]
        project: PathBuf,
    },

    /// Register rtk-mem-context.sh as PreToolUse:Task hook in ~/.claude/settings.json
    #[command(name = "install-hook")]
    InstallHook {
        /// Uninstall instead of install
        #[arg(long)]
        uninstall: bool,
        /// Show current hook status without changing anything
        #[arg(long)]
        status: bool,
    },

    /// Show token savings: raw source bytes vs compact context (E6.3)
    Gain {
        /// Project root to measure (default: current directory)
        #[arg(default_value = ".")]
        project: PathBuf,
    },

    /// E4.1: Start localhost HTTP API server (GET /v1/health, POST /v1/{explore,delta,refresh,context,plan-context})
    Serve {
        /// TCP port to listen on
        #[arg(long, default_value = "7700")]
        port: u16,
        /// Stop after N seconds with no requests (0 = run forever)
        #[arg(long, default_value = "300")]
        idle_secs: u64,
    },

    /// Ranked context plan for a specific task (budget-aware, deterministic)
    Plan {
        /// Task description (e.g. "fix jwt token refresh bug")
        task: String,
        /// Project root to index
        #[arg(default_value = ".")]
        project: PathBuf,
        /// Maximum token budget for context (0 = default 12000)
        #[arg(long, default_value = "12000")] // CHANGED: was 4000
        token_budget: u32,
        /// Output format: json (default), text, paths
        #[arg(short, long, default_value = "json")]
        format: String,
        /// Cap candidate count regardless of budget (for --format paths)
        #[arg(long, default_value = "25")]
        top: usize, // ADDED: --top N flag
        /// Force legacy (pre-graph-first) pipeline
        #[arg(long)]
        legacy: bool, // ADDED: PRD --legacy flag
        /// Print stage trace (graph seeds / semantic hits / final set)
        #[arg(long)]
        trace: bool, // ADDED: PRD --trace flag
    },

    /// Diagnose memory layer setup: hooks, cache, gain, rtk binary // T1
    Doctor {
        /// Project root to inspect (default: current directory)
        #[arg(default_value = ".")]
        project: PathBuf,
    },

    /// Idempotent installer: hooks + cache + doctor // T2
    Setup {
        /// Patch settings.json without interactive prompt
        #[arg(long)]
        auto_patch: bool,
        /// Skip starting file watchers
        #[arg(long)]
        no_watch: bool,
        /// Project root (default: current directory)
        #[arg(default_value = ".")]
        project: PathBuf,
    },

    /// Launch tmux dev environment: grepai watch + rtk memory watch + health loop // T5
    Devenv {
        /// Project root (default: current directory)
        #[arg(default_value = ".")]
        project: PathBuf,
        /// Debounce interval for rtk memory watch (seconds)
        #[arg(long, default_value = "2")]
        interval: u64,
        /// tmux session name
        #[arg(long, default_value = "rtk")] // [P2] fix: spec says "rtk" not "rtk-mem"
        session_name: String,
    },
}

fn run_npx_passthrough(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("npx requires a command argument");
    }

    let timer = tracking::TimedExecution::start();
    let mut cmd = std::process::Command::new("npx");
    for arg in args {
        cmd.arg(arg);
    }
    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }
    if verbose > 0 {
        eprintln!("Running: npx {}", args.join(" "));
    }

    let status = cmd.status().context("Failed to run npx command")?;
    let args_str = args.join(" ");
    timer.track_passthrough(
        &format!("npx {args_str}"),
        &format!("rtk npx {args_str} (passthrough)"),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

/// Select the best filter level for a file based on extension. // changed: smart default level helper
fn smart_read_level(path: &std::path::Path) -> crate::filter::FilterLevel {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "json" | "yaml" | "yml" | "toml" | "env" | "lock" | "mod" | "sum" | "csv" | "tsv"
        | "ini" | "cfg" | "conf" | "xml" => crate::filter::FilterLevel::None,
        _ => crate::filter::FilterLevel::Minimal,
    }
}

/// fix #200: RTK-only subcommands that should never fall back to raw execution.
/// Expanded: RTK-exclusive commands where system fallback is dangerous or wrong.
pub(crate) const RTK_META_COMMANDS: &[&str] = &[
    "gain",
    "discover",
    "learn",
    "init",
    "config",
    "proxy",
    "hook-audit",
    "cc-economics",
    // RTK-exclusive — no system equivalent or system cmd is dangerously different // changed
    "write",   // system write(1) sends terminal messages — very dangerous // changed
    "read",    // bash `read` reads from stdin — completely wrong // changed
    "rgai",    // no system equivalent // changed
    "memory",  // no system equivalent // changed
    "smart",   // no system equivalent // changed
    "summary", // no system equivalent // changed
    "rewrite", // fork: hook engine — fallback to system would be wrong
    "pipe",    // fork: stdin filter — no system equivalent
    "trust",   // fork: TOML filter trust — no system equivalent
    "untrust", // fork: TOML filter trust — no system equivalent
    "verify",  // fork: integrity + filter tests — no system equivalent
];

/// Print contextual fix hints when a write subcommand fails to parse. // changed
fn print_write_hint(args: &[String]) {
    // changed
    let sub = args.get(1).map(|s| s.as_str()).unwrap_or(""); // changed
    eprintln!(); // changed
    match sub {
        // changed
        "file" => {
            // changed
            eprintln!("  note: 'write file' creates NEW files only."); // changed
            eprintln!("        to modify existing files use 'write patch' or 'write replace':"); // changed
            eprintln!(); // changed
            eprintln!("    rtk write replace <file> --from <old> --to <new>"); // changed
            eprintln!("    rtk write patch   <file> --old <block> --new <block>");
            // changed
        } // changed
        "patch" | "replace" => {
            // changed
            eprintln!("  hint: shell metacharacters break inline --old/--new."); // changed
            eprintln!("        use @file refs via heredoc instead:"); // changed
            eprintln!(); // changed
            eprintln!("    rtk write file /tmp/rtk_old.txt --content @- << 'EOF'"); // changed
            eprintln!("    ...exact old block verbatim..."); // changed
            eprintln!("    EOF"); // changed
            eprintln!("    rtk write file /tmp/rtk_new.txt --content @- << 'EOF'"); // changed
            eprintln!("    ...new block..."); // changed
            eprintln!("    EOF"); // changed
            eprintln!("    rtk write patch <file> --old @/tmp/rtk_old.txt --new @/tmp/rtk_new.txt"); // changed
            eprintln!(); // changed
            eprintln!("  hint: if you get ERR_NO_MATCH, get exact bytes first:"); // changed
            eprintln!("    rtk read <file> --from <N> --to <M>   # copy verbatim into temp files");
            // changed
        } // changed
        "batch" => {
            // changed
            eprintln!("  hint: use @file refs inside the JSON plan for complex content:"); // changed
            eprintln!(); // changed
            eprintln!(r#"    rtk write batch --plan '["#); // changed
            eprintln!(
                r#"      {{"op":"patch","file":"src/lib.rs","old":"@/tmp/old.txt","new":"@/tmp/new.txt"}}"#
            ); // changed
            eprintln!(r#"    ]'"#); // changed
        } // changed
        _ => {
            // changed
            eprintln!("  hint: for multi-line content with special chars, use @file refs:"); // changed
            eprintln!(); // changed
            eprintln!("    rtk write file /tmp/rtk_old.txt --content @- << 'EOF'"); // changed
            eprintln!("    ...old block..."); // changed
            eprintln!("    EOF"); // changed
            eprintln!("    rtk write patch <file> --old @/tmp/rtk_old.txt --new @/tmp/rtk_new.txt");
            // changed
        } // changed
    } // changed
    eprintln!(); // changed
} // changed

/// fix #200: execute raw command when Clap parse fails (graceful fallback).
fn run_fallback(parse_error: clap::Error) -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        parse_error.exit();
    }

    // RTK meta-commands must never fall back — show Clap error directly.
    // For write subcommands, also print contextual hints before the Clap error. // changed
    if RTK_META_COMMANDS.contains(&args[0].as_str()) {
        if args[0] == "write" {
            // changed
            print_write_hint(&args); // changed
        } // changed
        parse_error.exit();
    }

    let raw_command = args.join(" ");
    let error_message = utils::strip_ansi(&parse_error.to_string());

    let timer = tracking::TimedExecution::start();

    // fork: TOML filter lookup (upstream v0.42.4) — bypass with RTK_NO_TOML=1.
    // Use basename of args[0] so absolute paths (/usr/bin/make) still match "^make\b".
    let lookup_cmd = {
        let base = std::path::Path::new(&args[0])
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| args[0].clone());
        std::iter::once(base.as_str())
            .chain(args[1..].iter().map(|s| s.as_str()))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let toml_match = if std::env::var("RTK_NO_TOML").ok().as_deref() == Some("1") {
        None
    } else {
        toml_filter::find_matching_filter(&lookup_cmd)
    };

    // A filter may declare invocation shapes it must not touch (see
    // `pass_through_if_args`); those fall through to the raw passthrough below
    // instead of being truncated.
    let toml_match = toml_match.filter(|filter| !filter.should_pass_through(&args[1..]));

    if let Some(filter) = toml_match {
        let result = if filter.filter_stderr {
            std::process::Command::new(&args[0])
                .args(&args[1..])
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
        } else {
            std::process::Command::new(&args[0])
                .args(&args[1..])
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit())
                .output()
        };

        match result {
            Ok(output) => {
                let exit_code = output.status.code().unwrap_or(1);
                let stdout_raw = String::from_utf8_lossy(&output.stdout);
                let stderr_raw = String::from_utf8_lossy(&output.stderr);
                let combined_raw = if filter.filter_stderr {
                    format!("{}{}", stdout_raw, stderr_raw)
                } else {
                    stdout_raw.to_string()
                };
                let success = output.status.success();
                let (filtered, loss) = toml_filter::apply_filter_with_info(filter, &combined_raw);
                let lossy = !matches!(loss, toml_filter::Lossiness::None);
                let hint = if !success {
                    crate::tee::tee_and_hint(&combined_raw, &raw_command, exit_code)
                } else {
                    match &loss {
                        toml_filter::Lossiness::None => None,
                        toml_filter::Lossiness::Tail {
                            tee_payload,
                            tail_offset,
                        } => {
                            crate::tee::force_tee_tail_hint(tee_payload, &raw_command, *tail_offset)
                        }
                        toml_filter::Lossiness::Whole => {
                            crate::tee::force_tee_hint(&combined_raw, &raw_command)
                        }
                    }
                };
                let candidate = if let Some(hint) = &hint {
                    format!("{}\n{}", filtered, hint)
                } else {
                    filtered
                };
                let shown = if lossy && hint.is_none() {
                    combined_raw.as_str()
                } else {
                    guard::never_worse(&combined_raw, &candidate)
                };
                println!("{}", shown);

                timer.track(
                    &raw_command,
                    &format!("rtk:toml {}", raw_command),
                    &combined_raw,
                    shown,
                );
                tracking::record_parse_failure_silent(&raw_command, &error_message, true);
                if exit_code != 0 {
                    std::process::exit(exit_code);
                }
                return Ok(());
            }
            Err(e) => {
                tracking::record_parse_failure_silent(&raw_command, &error_message, false);
                eprintln!("[rtk: {}]", e);
                std::process::exit(127);
            }
        }
    }

    // changed: name the raw command explicitly so agent sees what will run
    eprintln!(
        "[rtk: parse failed, running `{}` raw — use `rtk proxy` to silence this]",
        args[0]
    ); // changed

    let status = std::process::Command::new(&args[0])
        .args(&args[1..])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status();

    match status {
        Ok(s) => {
            timer.track_passthrough(&raw_command, &format!("rtk fallback: {}", raw_command));
            tracking::record_parse_failure_silent(&raw_command, &error_message, true);
            if !s.success() {
                std::process::exit(s.code().unwrap_or(1));
            }
        }
        Err(e) => {
            tracking::record_parse_failure_silent(&raw_command, &error_message, false);
            eprintln!("[rtk: fallback failed: {}]", e);
            parse_error.exit();
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    tracking::initialize_internal_evasion();

    #[cfg(unix)]
    // SAFETY: SIGPIPE is restored before threads or child processes start.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    // fix #200: graceful fallback when Clap cannot parse the command
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                e.exit();
            }
            return run_fallback(e);
        }
    };

    match cli.command {
        Commands::CapabilityBatch => run_capability_batch(std::io::stdin(), std::io::stdout())?,
        Commands::Ls { args } => {
            ls::run(&args, cli.verbose)?;
        }

        Commands::Tree { args } => {
            tree::run(&args, cli.verbose)?;
        }

        Commands::Read {
            file,
            additional_files,
            batch,
            max_tokens,
            per_file_tokens,
            level,
            from,
            to,
            max_lines,
            tail_lines,
            line_numbers,
            outline,
            symbols,
            changed,
            since,
            diff_context,
            dedup,
        } => {
            if batch {
                let mut incompatible = Vec::new();
                if level.is_some() {
                    incompatible.push("--level");
                }
                if from.is_some() {
                    incompatible.push("--from");
                }
                if to.is_some() {
                    incompatible.push("--to");
                }
                if max_lines.is_some() {
                    incompatible.push("--max-lines");
                }
                if tail_lines.is_some() {
                    incompatible.push("--tail-lines");
                }
                if line_numbers {
                    incompatible.push("--line-numbers");
                }
                if outline {
                    incompatible.push("--outline");
                }
                if symbols {
                    incompatible.push("--symbols");
                }
                if changed {
                    incompatible.push("--changed");
                }
                if since.is_some() {
                    incompatible.push("--since");
                }
                if dedup {
                    incompatible.push("--dedup");
                }
                if !incompatible.is_empty() {
                    anyhow::bail!("{} incompatible with --batch", incompatible.join(", "));
                }
                let mut files = Vec::with_capacity(additional_files.len() + 1);
                files.push(file);
                files.extend(additional_files);
                let max_tokens = max_tokens.ok_or_else(|| {
                    anyhow::anyhow!("--batch requires an explicit --max-tokens budget")
                })?;
                return read::run_batch(&files, max_tokens, per_file_tokens, cli.verbose);
            }
            if !additional_files.is_empty() {
                anyhow::bail!("multiple files require --batch and --max-tokens");
            }
            // Determine ReadMode from flags
            let mode = if outline {
                read::ReadMode::Outline
            } else if symbols {
                read::ReadMode::Symbols
            } else if changed {
                read::ReadMode::Changed
            } else if let Some(rev) = since {
                read::ReadMode::Since(rev)
            } else {
                read::ReadMode::Full
            };

            // Reject incompatible flags in non-full modes.
            if !matches!(mode, read::ReadMode::Full) {
                let mut incompatible = Vec::new();
                if from.is_some() {
                    incompatible.push("--from");
                }
                if to.is_some() {
                    incompatible.push("--to");
                }
                if max_lines.is_some() {
                    incompatible.push("--max-lines");
                }
                if tail_lines.is_some() {
                    incompatible.push("--tail-lines");
                }
                if line_numbers {
                    incompatible.push("-n/--line-numbers");
                }
                if dedup {
                    incompatible.push("--dedup");
                }
                if level.map_or(false, |l| l != filter::FilterLevel::Minimal) {
                    // changed: Option<FilterLevel>
                    incompatible.push("--level");
                }
                if matches!(mode, read::ReadMode::Outline | read::ReadMode::Symbols)
                    && diff_context != 3
                {
                    incompatible.push("--diff-context");
                }
                if !incompatible.is_empty() {
                    let mode_flag = match &mode {
                        read::ReadMode::Outline => "--outline",
                        read::ReadMode::Symbols => "--symbols",
                        read::ReadMode::Changed => "--changed",
                        read::ReadMode::Since(_) => "--since",
                        read::ReadMode::Full => unreachable!(),
                    };
                    anyhow::bail!(
                        "{} incompatible with {}",
                        incompatible.join(", "),
                        mode_flag
                    );
                }
            }

            // Dispatch by mode
            match mode {
                read::ReadMode::Outline | read::ReadMode::Symbols => {
                    if file == Path::new("-") {
                        anyhow::bail!("--outline and --symbols require a file path, not stdin");
                    }
                    read::run_symbols(&file, &mode, cli.verbose)?;
                }
                read::ReadMode::Changed => {
                    if file == Path::new("-") {
                        anyhow::bail!("--changed requires a file path, not stdin");
                    }
                    read::run_changed(&file, None, diff_context, cli.verbose)?;
                }
                read::ReadMode::Since(ref rev) => {
                    if file == Path::new("-") {
                        anyhow::bail!("--since requires a file path, not stdin");
                    }
                    read::run_changed(&file, Some(rev), diff_context, cli.verbose)?;
                }
                read::ReadMode::Full => {
                    // Resolve smart default level (US-007) // changed: auto-select level
                    let level = level.unwrap_or_else(|| {
                        if from.is_some()
                            || to.is_some()
                            || max_lines.is_some()
                            || tail_lines.is_some()
                            || line_numbers
                        {
                            filter::FilterLevel::None // range = edit mode → full content
                        } else {
                            smart_read_level(&file) // extension-based
                        }
                    });
                    if file == Path::new("-") {
                        read::run_stdin(
                            level,
                            from,
                            to,
                            max_lines,
                            tail_lines,
                            line_numbers,
                            cli.verbose,
                        )?;
                    } else {
                        read::run(
                            &file,
                            level,
                            from,
                            to,
                            max_lines,
                            tail_lines,
                            line_numbers,
                            dedup,
                            cli.verbose,
                        )?;
                    }
                }
            }
        }

        Commands::Smart {
            file,
            model,
            force_download,
        } => {
            local_llm::run(&file, &model, force_download, cli.verbose)?;
        }

        Commands::Git {
            directory,
            config_override,
            git_dir,
            work_tree,
            no_pager,
            no_optional_locks,
            bare,
            literal_pathspecs,
            command,
        } => {
            // Build global git args (inserted between "git" and subcommand)
            let mut global_args: Vec<String> = Vec::new();
            for dir in &directory {
                global_args.push("-C".to_string());
                global_args.push(dir.clone());
            }
            for cfg in &config_override {
                global_args.push("-c".to_string());
                global_args.push(cfg.clone());
            }
            if let Some(ref dir) = git_dir {
                global_args.push("--git-dir".to_string());
                global_args.push(dir.clone());
            }
            if let Some(ref tree) = work_tree {
                global_args.push("--work-tree".to_string());
                global_args.push(tree.clone());
            }
            if no_pager {
                global_args.push("--no-pager".to_string());
            }
            if no_optional_locks {
                global_args.push("--no-optional-locks".to_string());
            }
            if bare {
                global_args.push("--bare".to_string());
            }
            if literal_pathspecs {
                global_args.push("--literal-pathspecs".to_string());
            }

            match command {
                GitCommands::Diff { args } => {
                    git::run(
                        git::GitCommand::Diff,
                        &args,
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Log { args } => {
                    git::run(git::GitCommand::Log, &args, None, cli.verbose, &global_args)?;
                }
                GitCommands::Status { args } => {
                    git::run(
                        git::GitCommand::Status,
                        &args,
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Show { args } => {
                    git::run(
                        git::GitCommand::Show,
                        &args,
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Add { args } => {
                    git::run(git::GitCommand::Add, &args, None, cli.verbose, &global_args)?;
                }
                GitCommands::Commit { message } => {
                    git::run(
                        git::GitCommand::Commit { message },
                        &[],
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Checkout { args } => {
                    git::run(
                        git::GitCommand::Checkout,
                        &args,
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Push { args } => {
                    git::run(
                        git::GitCommand::Push,
                        &args,
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Pull { args } => {
                    git::run(
                        git::GitCommand::Pull,
                        &args,
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Branch { args } => {
                    git::run(
                        git::GitCommand::Branch,
                        &args,
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Fetch { args } => {
                    git::run(
                        git::GitCommand::Fetch,
                        &args,
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Stash { subcommand, args } => {
                    git::run(
                        git::GitCommand::Stash { subcommand },
                        &args,
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Worktree { args } => {
                    git::run(
                        git::GitCommand::Worktree,
                        &args,
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Blame { args } => {
                    git::run(
                        git::GitCommand::Blame,
                        &args,
                        None,
                        cli.verbose,
                        &global_args,
                    )?;
                }
                GitCommands::Other(args) => {
                    git::run_passthrough(&args, cli.verbose, &global_args)?;
                }
            }
        }

        Commands::Write { output, command } => match command {
            WriteCommands::Replace {
                file,
                from,
                to,
                all,
                dry_run,
                fast,
                cas, // changed: concurrency flags
                retry,
            } => {
                let concurrency = write_cmd::ConcurrencyOpts {
                    cas,
                    max_retries: retry,
                }; // changed: construct ConcurrencyOpts
                let params = write_cmd::WriteParams {
                    dry_run,
                    fast,
                    verbose: cli.verbose,
                    output,
                    concurrency,
                };
                // expand @file refs so multi-line/special-char content can bypass shell escaping
                let from = write_cmd::expand_at_ref(&from)?;
                let to = write_cmd::expand_at_ref(&to)?;
                write_cmd::run_replace(&file, &from, &to, all, params)?;
            }
            WriteCommands::Patch {
                file,
                old,
                new_text,
                all,
                dry_run,
                fast,
                cas, // changed: concurrency flags
                retry,
            } => {
                let concurrency = write_cmd::ConcurrencyOpts {
                    cas,
                    max_retries: retry,
                }; // changed: construct ConcurrencyOpts
                let params = write_cmd::WriteParams {
                    dry_run,
                    fast,
                    verbose: cli.verbose,
                    output,
                    concurrency,
                };
                // expand @file refs so multi-line/special-char content can bypass shell escaping
                let old = write_cmd::expand_at_ref(&old)?;
                let new_text = write_cmd::expand_at_ref(&new_text)?;
                write_cmd::run_patch(&file, &old, &new_text, all, params)?;
            }
            WriteCommands::Set {
                file,
                key,
                value,
                value_type,
                format,
                dry_run,
                fast,
                cas, // changed: concurrency flags
                retry,
            } => {
                let concurrency = write_cmd::ConcurrencyOpts {
                    cas,
                    max_retries: retry,
                }; // changed: construct ConcurrencyOpts
                let params = write_cmd::WriteParams {
                    dry_run,
                    fast,
                    verbose: cli.verbose,
                    output,
                    concurrency,
                };
                write_cmd::run_set(&file, &key, &value, value_type, format, params)?;
            }
            WriteCommands::Batch {
                plan,
                dry_run,
                fast,
                cas, // changed: concurrency flags
                retry,
            } => {
                let concurrency = write_cmd::ConcurrencyOpts {
                    cas,
                    max_retries: retry,
                }; // changed: construct ConcurrencyOpts
                let params = write_cmd::WriteParams {
                    dry_run,
                    fast,
                    verbose: cli.verbose,
                    output,
                    concurrency,
                };
                let plan = write_cmd::expand_at_ref(&plan)?.into_owned(); // changed: support @file/@- for --plan (bypasses shell escaping for complex content)
                write_cmd::run_batch(&plan, params)?;
            }
            // changed: new create subcommand handler
            WriteCommands::Create {
                file,
                content,
                force,
                dry_run,
                fast,
            } => {
                let params = write_cmd::WriteParams {
                    dry_run,
                    fast,
                    verbose: cli.verbose,
                    output,
                    concurrency: write_cmd::ConcurrencyOpts::default(),
                };
                write_cmd::run_create(&file, &content, force, params)?;
            }
        },

        Commands::Build { command } => match command {
            BuildCommands::Sh {
                root,
                no_debug,
                no_release,
                skip_user,
                skip_usr_local,
                symlink_usr_local,
                set_version,
                no_verify,
                no_sudo,
                no_init_sync,
            } => {
                let use_sudo =
                    !no_sudo && std::env::var("RTK_BUILD_NO_SUDO").ok().as_deref() != Some("1");
                build_cmd::run_sh(
                    build_cmd::BuildShOptions {
                        root,
                        build_debug: !no_debug,
                        build_release: !no_release,
                        install_user: !skip_user,
                        install_usr_local: !skip_usr_local,
                        verify: !no_verify,
                        symlink_usr_local,
                        use_sudo,
                        set_version,
                        sync_global_init: !no_init_sync,
                    },
                    cli.verbose,
                )?;
            }
        },

        Commands::Gh { subcommand, args } => {
            gh_cmd::run(&subcommand, &args, cli.verbose, cli.ultra_compact)?;
        }

        Commands::Pnpm { command } => match command {
            PnpmCommands::List { depth, args } => {
                pnpm_cmd::run(pnpm_cmd::PnpmCommand::List { depth }, &args, cli.verbose)?;
            }
            PnpmCommands::Outdated { args } => {
                pnpm_cmd::run(pnpm_cmd::PnpmCommand::Outdated, &args, cli.verbose)?;
            }
            PnpmCommands::Install { packages, args } => {
                pnpm_cmd::run(
                    pnpm_cmd::PnpmCommand::Install { packages },
                    &args,
                    cli.verbose,
                )?;
            }
            PnpmCommands::Build { args } => {
                next_cmd::run(&args, cli.verbose)?;
            }
            PnpmCommands::Typecheck { args } => {
                tsc_cmd::run(&args, cli.verbose)?;
            }
            PnpmCommands::Other(args) => {
                pnpm_cmd::run_passthrough(&args, cli.verbose)?;
            }
        },

        Commands::Err { command } => {
            runner::run_err(&command, cli.verbose)?;
        }

        Commands::Test { command } => {
            runner::run_test(&command, cli.verbose)?;
        }

        Commands::Json { file, depth } => {
            if file == Path::new("-") {
                json_cmd::run_stdin(depth, cli.verbose)?;
            } else {
                json_cmd::run(&file, depth, cli.verbose)?;
            }
        }

        Commands::Deps { path } => {
            deps::run(&path, cli.verbose)?;
        }

        Commands::Env { filter, show_all } => {
            env_cmd::run(filter.as_deref(), show_all, cli.verbose)?;
        }

        Commands::Find { args } => {
            find_cmd::run_from_args(&args, cli.verbose)?; // fix #211: native flag support
        }

        Commands::Diff { file1, file2 } => {
            if let Some(f2) = file2 {
                diff_cmd::run(&file1, &f2, cli.verbose)?;
            } else {
                diff_cmd::run_stdin(cli.verbose)?;
            }
        }

        Commands::Log { file } => {
            if let Some(f) = file {
                log_cmd::run_file(&f, cli.verbose)?;
            } else {
                log_cmd::run_stdin(cli.verbose)?;
            }
        }

        Commands::Docker { command } => match command {
            DockerCommands::Ps => {
                container::run(container::ContainerCmd::DockerPs, &[], cli.verbose)?;
            }
            DockerCommands::Images => {
                container::run(container::ContainerCmd::DockerImages, &[], cli.verbose)?;
            }
            DockerCommands::Logs { container: c } => {
                container::run(container::ContainerCmd::DockerLogs, &[c], cli.verbose)?;
            }
            // upstream sync 0.21.0: docker compose routing
            DockerCommands::Compose { command: compose } => match compose {
                ComposeCommands::Ps => {
                    container::run_compose_ps(cli.verbose)?;
                }
                ComposeCommands::Logs { service } => {
                    container::run_compose_logs(service.as_deref(), cli.verbose)?;
                }
                ComposeCommands::Build { service } => {
                    container::run_compose_build(service.as_deref(), cli.verbose)?;
                }
                ComposeCommands::Other(args) => {
                    container::run_compose_passthrough(&args, cli.verbose)?;
                }
            },
            DockerCommands::Other(args) => {
                container::run_docker_passthrough(&args, cli.verbose)?;
            }
        },

        Commands::Kubectl { command } => match command {
            KubectlCommands::Get { args } => {
                container::run_kubectl_get(&args, cli.verbose)?;
            }
            KubectlCommands::Pods { namespace, all } => {
                let mut args: Vec<String> = Vec::new();
                if all {
                    args.push("-A".to_string());
                } else if let Some(n) = namespace {
                    args.push("-n".to_string());
                    args.push(n);
                }
                container::run(container::ContainerCmd::KubectlPods, &args, cli.verbose)?;
            }
            KubectlCommands::Services { namespace, all } => {
                let mut args: Vec<String> = Vec::new();
                if all {
                    args.push("-A".to_string());
                } else if let Some(n) = namespace {
                    args.push("-n".to_string());
                    args.push(n);
                }
                container::run(container::ContainerCmd::KubectlServices, &args, cli.verbose)?;
            }
            KubectlCommands::Logs { pod, container: c } => {
                let mut args = vec![pod];
                if let Some(cont) = c {
                    args.push("-c".to_string());
                    args.push(cont);
                }
                container::run(container::ContainerCmd::KubectlLogs, &args, cli.verbose)?;
            }
            KubectlCommands::Other(args) => {
                container::run_kubectl_passthrough(&args, cli.verbose)?;
            }
        },

        Commands::Oc { command } => match command {
            OcCommands::Get { args } => {
                container::run_oc_get(&args, cli.verbose)?;
            }
            OcCommands::Pods { namespace, all } => {
                let mut args = Vec::new();
                if all {
                    args.push("-A".to_string());
                } else if let Some(namespace) = namespace {
                    args.extend(["-n".to_string(), namespace]);
                }
                container::k8s_pods("oc", &args, cli.verbose)?;
            }
            OcCommands::Services { namespace, all } => {
                let mut args = Vec::new();
                if all {
                    args.push("-A".to_string());
                } else if let Some(namespace) = namespace {
                    args.extend(["-n".to_string(), namespace]);
                }
                container::k8s_services("oc", &args, cli.verbose)?;
            }
            OcCommands::Logs { pod, container: c } => {
                let mut args = vec![pod];
                if let Some(container) = c {
                    args.extend(["-c".to_string(), container]);
                }
                container::k8s_logs("oc", &args, cli.verbose)?;
            }
            OcCommands::Other(args) => {
                container::run_oc_passthrough(&args, cli.verbose)?;
            }
        },

        Commands::Summary { command } => {
            let cmd = command.join(" ");
            summary::run(&cmd, cli.verbose)?;
        }

        Commands::Grep {
            max_len,
            max,
            context_only,
            file_type: _,
            extra_args,
        } => {
            let code = search::run(
                search::Engine::Grep,
                max_len,
                max,
                context_only,
                &extra_args,
                cli.verbose,
            )?;
            if code != 0 {
                std::process::exit(code);
            }
        }
        Commands::Rg { extra_args } => {
            let code = search::run(search::Engine::Rg, 80, 200, false, &extra_args, cli.verbose)?;
            if code != 0 {
                std::process::exit(code);
            }
        }

        Commands::Rgai {
            query,
            path,
            max,
            context,
            file_type,
            max_file_kb,
            json,
            compact,
            builtin, // --builtin flag: skip grepai delegation
            literal, // fork: verbatim, case-sensitive lookup
            files,   // ADDED: --files flag
            project_root,
        } => {
            // Backward-compat: rtk rgai "query words" ./src -> path="./src"
            let (query, path) = normalize_rgai_args(query, path);
            rgai_cmd::run(
                &query,
                rgai_cmd::RgaiOptions {
                    path: &path,
                    project_root: project_root.as_deref(),
                    max_results: max,
                    context_lines: context,
                    file_type: file_type.as_deref(),
                    max_file_kb,
                    json_output: json,
                    compact,
                    builtin,
                    literal,
                    files: files.as_deref(),
                    verbose: cli.verbose,
                },
            )?;
        }

        Commands::Init {
            global,
            show,
            claude_md,
            hook_only,
            auto_patch,
            no_patch,
            uninstall,
            codex,
            trust_filters,
            no_trust_filters,
            dry_run,
        } => {
            if show {
                init::show_config(codex)?;
            } else if uninstall {
                init::uninstall(global, codex, dry_run, cli.verbose)?;
            } else {
                let patch_mode = if auto_patch {
                    init::PatchMode::Auto
                } else if no_patch {
                    init::PatchMode::Skip
                } else {
                    init::PatchMode::Ask
                };
                init::run(
                    global,
                    claude_md,
                    hook_only,
                    codex,
                    patch_mode,
                    dry_run,
                    cli.verbose,
                )?;
                let filter_trust = if trust_filters {
                    init::FilterTrust::Trust
                } else if no_trust_filters || auto_patch {
                    init::FilterTrust::Skip
                } else {
                    init::FilterTrust::Ask
                };
                init::finalize_filter_trust(dry_run, filter_trust)?;
            }
        }

        Commands::Wget { url, stdout, args } => {
            if stdout {
                wget_cmd::run_stdout(&url, &args, cli.verbose)?;
            } else {
                wget_cmd::run(&url, &args, cli.verbose)?;
            }
        }

        Commands::Wc { args } => {
            wc_cmd::run(&args, cli.verbose)?;
        }

        Commands::Gain {
            project, // added
            graph,
            history,
            quota,
            tier,
            daily,
            weekly,
            monthly,
            all,
            format,
            failures, // fix #200
        } => {
            gain::run(
                project, // added: pass project flag
                graph,
                history,
                quota,
                &tier,
                daily,
                weekly,
                monthly,
                all,
                &format,
                failures, // fix #200
                cli.verbose,
            )?;
        }

        Commands::CcEconomics {
            daily,
            weekly,
            monthly,
            all,
            format,
        } => {
            cc_economics::run(daily, weekly, monthly, all, &format, cli.verbose)?;
        }

        Commands::Config { create, format } => {
            if create {
                let path = config::Config::create_default()?;
                println!("Created: {}", path.display());
            } else {
                config::show_config(&format)?;
            }
        }

        Commands::Vitest { command } => match command {
            VitestCommands::Run { args } => {
                vitest_cmd::run(vitest_cmd::VitestCommand::Run, &args, cli.verbose)?;
            }
        },

        Commands::Prisma { command } => match command {
            PrismaCommands::Generate { args } => {
                prisma_cmd::run(prisma_cmd::PrismaCommand::Generate, &args, cli.verbose)?;
            }
            PrismaCommands::Migrate { command } => match command {
                PrismaMigrateCommands::Dev { name, args } => {
                    prisma_cmd::run(
                        prisma_cmd::PrismaCommand::Migrate {
                            subcommand: prisma_cmd::MigrateSubcommand::Dev { name },
                        },
                        &args,
                        cli.verbose,
                    )?;
                }
                PrismaMigrateCommands::Status { args } => {
                    prisma_cmd::run(
                        prisma_cmd::PrismaCommand::Migrate {
                            subcommand: prisma_cmd::MigrateSubcommand::Status,
                        },
                        &args,
                        cli.verbose,
                    )?;
                }
                PrismaMigrateCommands::Deploy { args } => {
                    prisma_cmd::run(
                        prisma_cmd::PrismaCommand::Migrate {
                            subcommand: prisma_cmd::MigrateSubcommand::Deploy,
                        },
                        &args,
                        cli.verbose,
                    )?;
                }
            },
            PrismaCommands::DbPush { args } => {
                prisma_cmd::run(prisma_cmd::PrismaCommand::DbPush, &args, cli.verbose)?;
            }
        },

        Commands::Tsc { args } => {
            tsc_cmd::run(&args, cli.verbose)?;
        }

        Commands::Next { args } => {
            next_cmd::run(&args, cli.verbose)?;
        }

        Commands::Lint { args } => {
            lint_cmd::run(&args, cli.verbose)?;
        }

        Commands::Prettier { args } => {
            prettier_cmd::run(&args, cli.verbose)?;
        }

        Commands::Format { args } => {
            format_cmd::run(&args, cli.verbose)?;
        }

        Commands::Playwright { args } => {
            playwright_cmd::run(&args, cli.verbose)?;
        }

        Commands::Cargo { command } => match command {
            CargoCommands::Build { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Build, &args, cli.verbose)?;
            }
            CargoCommands::Test { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Test, &args, cli.verbose)?;
            }
            CargoCommands::Clippy { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Clippy, &args, cli.verbose)?;
            }
            CargoCommands::Check { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Check, &args, cli.verbose)?;
            }
            CargoCommands::Install { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Install, &args, cli.verbose)?;
            }
            CargoCommands::Nextest { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Nextest, &args, cli.verbose)?;
            }
            CargoCommands::Other(args) => {
                cargo_cmd::run_passthrough(&args, cli.verbose)?;
            }
        },

        Commands::Npm { args } => {
            npm_cmd::run(&args, cli.verbose, cli.skip_env)?;
        }

        Commands::Curl { args } => {
            curl_cmd::run(&args, cli.verbose)?;
        }

        Commands::Ssh { args } => {
            ssh_cmd::run(&args, cli.verbose)?;
        }

        Commands::Lsof { args } => {
            lsof_cmd::run(&args, cli.verbose)?;
        }

        Commands::Ps { args } => {
            ps_cmd::run(&args, cli.verbose)?;
        }

        Commands::Sqlite3 {
            database,
            query,
            columns,
            max_rows,
            max_tokens,
        } => {
            sqlite_cmd::run(
                &database,
                &query,
                &columns,
                max_rows,
                max_tokens,
                cli.verbose,
            )?;
        }

        Commands::Tar { max_entries, args } => {
            tar_cmd::run(&args, max_entries, cli.verbose)?;
        }

        Commands::Logs {
            host,
            container,
            tail,
            since,
            timestamps,
        } => {
            remote_logs_cmd::run(
                &host,
                &container,
                tail,
                since.as_deref(),
                timestamps,
                cli.verbose,
            )?;
        }

        Commands::Discover {
            project,
            limit,
            all,
            since,
            format,
        } => {
            discover::run(project.as_deref(), all, since, limit, &format, cli.verbose)?;
        }

        Commands::Memory { command } => match command {
            MemoryCommands::Explore {
                project,
                refresh,
                strict, // P1: strict dirty-blocking
                detail,
                format,
                query_type, // E2.3
            } => {
                memory_layer::run_explore(
                    &project,
                    refresh,
                    strict,
                    detail,
                    &format,
                    query_type,
                    cli.verbose,
                )?;
            }
            MemoryCommands::Delta {
                project,
                since,
                detail,
                format,
                query_type, // E2.3
            } => {
                memory_layer::run_delta(
                    &project,
                    since.as_deref(),
                    detail,
                    &format,
                    query_type,
                    cli.verbose,
                )?;
            }
            MemoryCommands::Refresh {
                project,
                detail,
                format,
                query_type, // E2.3
            } => {
                memory_layer::run_refresh(&project, detail, &format, query_type, cli.verbose)?;
            }
            MemoryCommands::Watch {
                project,
                interval,
                detail,
                format,
                query_type, // E2.3
            } => {
                memory_layer::run_watch(
                    &project,
                    interval,
                    detail,
                    &format,
                    query_type,
                    cli.verbose,
                )?;
            }
            MemoryCommands::Status { project } => {
                memory_layer::run_status(&project, cli.verbose)?;
            }
            MemoryCommands::Clear { project } => {
                memory_layer::run_clear(&project, cli.verbose)?;
            }
            MemoryCommands::InstallHook { uninstall, status } => {
                memory_layer::run_install_hook(uninstall, status, cli.verbose)?;
            }
            MemoryCommands::Gain { project } => {
                // E6.3
                memory_layer::run_gain(&project, cli.verbose)?;
            }
            MemoryCommands::Serve { port, idle_secs } => {
                // E4.1: HTTP API daemon
                memory_layer::run_serve(port, idle_secs, cli.verbose)?;
            }

            MemoryCommands::Plan {
                task,
                project,
                token_budget,
                format,
                top,    // ADDED: --top N flag
                legacy, // ADDED: PRD --legacy flag
                trace,  // ADDED: PRD --trace flag
            } => {
                // Plan-context: ranked candidates under token budget
                memory_layer::run_plan(
                    &project,
                    &task,
                    token_budget,
                    &format,
                    top,
                    legacy,
                    trace,
                    cli.verbose,
                )?; // CHANGED: pass legacy/trace
            }
            MemoryCommands::Doctor { project } => {
                // T1
                memory_layer::run_doctor(&project, cli.verbose)?;
            }
            MemoryCommands::Setup {
                auto_patch,
                no_watch,
                project,
            } => {
                // T2
                memory_layer::run_setup(&project, auto_patch, no_watch, cli.verbose)?;
            }
            MemoryCommands::Devenv {
                project,
                interval,
                session_name,
            } => {
                // T5
                memory_layer::run_devenv(&project, interval, &session_name, cli.verbose)?;
            }
        },

        Commands::Learn {
            project,
            all,
            since,
            format,
            write_rules,
            min_confidence,
            min_occurrences,
        } => {
            learn::run(
                project,
                all,
                since,
                format,
                write_rules,
                min_confidence,
                min_occurrences,
            )?;
        }

        Commands::Npx { args } => {
            if args.is_empty() {
                anyhow::bail!("npx requires a command argument");
            }

            // Intelligent routing: delegate to specialized filters
            match args[0].as_str() {
                "tsc" | "typescript" => {
                    tsc_cmd::run(&args[1..], cli.verbose)?;
                }
                "vue-tsc" => {
                    tsc_cmd::run_vue_tsc(&args[1..], cli.verbose, cli.skip_env)?;
                }
                "eslint" => {
                    lint_cmd::run(&args[1..], cli.verbose)?;
                }
                "prisma" => {
                    // Route to prisma_cmd based on subcommand
                    if args.len() > 1 {
                        let prisma_args: Vec<String> = args[2..].to_vec();
                        match args[1].as_str() {
                            "generate" => {
                                prisma_cmd::run(
                                    prisma_cmd::PrismaCommand::Generate,
                                    &prisma_args,
                                    cli.verbose,
                                )?;
                            }
                            "db" if args.len() > 2 && args[2] == "push" => {
                                prisma_cmd::run(
                                    prisma_cmd::PrismaCommand::DbPush,
                                    &args[3..],
                                    cli.verbose,
                                )?;
                            }
                            _ => {
                                run_npx_passthrough(&args, cli.verbose, cli.skip_env)?;
                            }
                        }
                    } else {
                        run_npx_passthrough(&args, cli.verbose, cli.skip_env)?;
                    }
                }
                "next" => {
                    next_cmd::run(&args[1..], cli.verbose)?;
                }
                "prettier" => {
                    prettier_cmd::run(&args[1..], cli.verbose)?;
                }
                "playwright" => {
                    playwright_cmd::run(&args[1..], cli.verbose)?;
                }
                _ => {
                    // Generic npx passthrough (keep tool semantics; no npm run fallback).
                    run_npx_passthrough(&args, cli.verbose, cli.skip_env)?;
                }
            }
        }

        Commands::Bun { args } => {
            bun_cmd::run(&args, cli.verbose, cli.skip_env)?;
        }

        Commands::Ruff { args } => {
            ruff_cmd::run(&args, cli.verbose)?;
        }

        Commands::Pytest { args } => {
            pytest_cmd::run(&args, cli.verbose)?;
        }

        Commands::Pip { args } => {
            pip_cmd::run(&args, cli.verbose)?;
        }

        Commands::Go { command } => match command {
            GoCommands::Test { args } => {
                go_cmd::run_test(&args, cli.verbose)?;
            }
            GoCommands::Build { args } => {
                go_cmd::run_build(&args, cli.verbose)?;
            }
            GoCommands::Vet { args } => {
                go_cmd::run_vet(&args, cli.verbose)?;
            }
            GoCommands::Run { args } => {
                go_cmd::run_run(&args, cli.verbose)?;
            }
            GoCommands::Other(args) => {
                go_cmd::run_other(&args, cli.verbose)?;
            }
        },

        Commands::GolangciLint { args } => {
            golangci_cmd::run(&args, cli.verbose)?;
        }

        Commands::HookAudit { since } => {
            // upstream sync: hook audit command
            hook_audit_cmd::run(since, cli.verbose)?;
        }

        // fork: ported from upstream v0.42.4
        Commands::Aws { subcommand, args } => aws_cmd::run(&subcommand, &args, cli.verbose)?,

        Commands::Psql { args } => psql_cmd::run(&args, cli.verbose)?,

        Commands::Mypy { args } => mypy_cmd::run(&args, cli.verbose)?,

        Commands::Gt { command } => match command {
            GtCommands::Log { args } => gt_cmd::run_log(&args, cli.verbose)?,
            GtCommands::Submit { args } => gt_cmd::run_submit(&args, cli.verbose)?,
            GtCommands::Sync { args } => gt_cmd::run_sync(&args, cli.verbose)?,
            GtCommands::Restack { args } => gt_cmd::run_restack(&args, cli.verbose)?,
            GtCommands::Create { args } => gt_cmd::run_create(&args, cli.verbose)?,
            GtCommands::Branch { args } => gt_cmd::run_branch(&args, cli.verbose)?,
            GtCommands::Other(args) => gt_cmd::run_other(&args, cli.verbose)?,
        },

        Commands::Pipe {
            filter,
            passthrough,
        } => {
            pipe_cmd::run(filter.as_deref(), passthrough)?;
        }

        Commands::Trust { list } => {
            trust::run_trust(list)?;
        }

        Commands::Untrust => {
            trust::run_untrust()?;
        }

        Commands::Verify {
            filter,
            require_all,
        } => {
            if filter.is_some() {
                verify_cmd::run(filter, require_all)?;
            } else {
                integrity::run_verify(cli.verbose)?;
                verify_cmd::run(None, require_all)?;
            }
        }

        Commands::Rewrite { args } => {
            let cmd = args.join(" ");
            rewrite_cmd::run(&cmd)?;
        }

        Commands::RewritePlan { args } => {
            let cmd = args.join(" ");
            rewrite_cmd::run_plan(&cmd)?;
        }

        Commands::Proxy { args } => {
            // fix #268: streaming output — spawn() + threads instead of output() (buffered)
            use std::io::{Read, Write};
            use std::process::{Command, Stdio};
            use std::sync::{Arc, Mutex};
            use std::thread;

            if args.is_empty() {
                anyhow::bail!(
                    "proxy requires a command to execute\nUsage: rtk proxy <command> [args...]"
                );
            }

            let timer = tracking::TimedExecution::start();

            let cmd_name = args[0].to_string_lossy();
            let cmd_args: Vec<String> = args[1..]
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();

            if cli.verbose > 0 {
                eprintln!("Proxy mode: {} {}", cmd_name, cmd_args.join(" "));
            }

            // fix #268: spawn child with piped stdio for real-time streaming
            let mut child = Command::new(cmd_name.as_ref())
                .args(&cmd_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context(format!("Failed to execute command: {}", cmd_name))?;

            let stdout_capture = Arc::new(Mutex::new(String::new()));
            let stderr_capture = Arc::new(Mutex::new(String::new()));

            // Thread: forward stdout chunks in real-time while capturing for tracking
            let stdout_handle = child.stdout.take().map(|stdout| {
                let capture = Arc::clone(&stdout_capture);
                thread::spawn(move || -> std::io::Result<()> {
                    let mut reader = std::io::BufReader::new(stdout);
                    let mut out = std::io::stdout();
                    let mut buf = [0u8; 8192];
                    loop {
                        let n = reader.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        out.write_all(&buf[..n])?;
                        out.flush()?;
                        let chunk = String::from_utf8_lossy(&buf[..n]);
                        if let Ok(mut captured) = capture.lock() {
                            captured.push_str(&chunk);
                        }
                    }
                    Ok(())
                })
            });

            // Thread: forward stderr chunks in real-time while capturing for tracking
            let stderr_handle = child.stderr.take().map(|stderr| {
                let capture = Arc::clone(&stderr_capture);
                thread::spawn(move || -> std::io::Result<()> {
                    let mut reader = std::io::BufReader::new(stderr);
                    let mut err = std::io::stderr();
                    let mut buf = [0u8; 8192];
                    loop {
                        let n = reader.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        err.write_all(&buf[..n])?;
                        err.flush()?;
                        let chunk = String::from_utf8_lossy(&buf[..n]);
                        if let Ok(mut captured) = capture.lock() {
                            captured.push_str(&chunk);
                        }
                    }
                    Ok(())
                })
            });

            let status = child.wait().context("Failed waiting for proxy command")?;

            if let Some(handle) = stdout_handle {
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("stdout reader thread panicked"))?
                    .context("Failed reading proxy stdout")?;
            }
            if let Some(handle) = stderr_handle {
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("stderr reader thread panicked"))?
                    .context("Failed reading proxy stderr")?;
            }

            let stdout = stdout_capture.lock().map(|s| s.clone()).unwrap_or_default();
            let stderr = stderr_capture.lock().map(|s| s.clone()).unwrap_or_default();
            let full_output = format!("{}{}", stdout, stderr);

            // Track usage (input = output since no filtering)
            timer.track(
                &format!("{} {}", cmd_name, cmd_args.join(" ")),
                &format!("rtk proxy {} {}", cmd_name, cmd_args.join(" ")),
                &full_output,
                &full_output,
            );

            // Exit with same code as child process
            if !status.success() {
                std::process::exit(crate::stream::status_to_exit_code(status));
            }
        }
    }

    Ok(())
}

fn run_capability_batch(mut input: impl Read, mut output: impl Write) -> Result<()> {
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(CAPABILITY_BATCH_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("failed to read capability batch")?;
    if bytes.len() as u64 > CAPABILITY_BATCH_MAX_BYTES {
        anyhow::bail!("capability batch exceeds {CAPABILITY_BATCH_MAX_BYTES} bytes");
    }
    let request: CapabilityBatchRequest =
        serde_json::from_slice(&bytes).context("invalid capability batch JSON")?;
    if request.commands.len() > CAPABILITY_BATCH_MAX_COMMANDS {
        anyhow::bail!("capability batch exceeds {CAPABILITY_BATCH_MAX_COMMANDS} commands");
    }
    let supported = request
        .commands
        .iter()
        .map(|command| discover::registry::has_existing_route(command))
        .collect();
    serde_json::to_writer(&mut output, &CapabilityBatchResponse { supported })
        .context("failed to write capability batch JSON")?;
    output
        .write_all(b"\n")
        .context("failed to finish capability batch JSON")?;
    Ok(())
}

/// Normalize rgai positional args: detect trailing path token in query words.
fn normalize_rgai_args(mut query_parts: Vec<String>, mut path: String) -> (String, String) {
    if path == "." && query_parts.len() > 1 {
        if let Some(last) = query_parts.last().cloned() {
            if looks_like_path_token(&last) {
                path = last;
                query_parts.pop();
            }
        }
    }
    let query = query_parts.join(" ");
    (query, path)
}

fn looks_like_path_token(token: &str) -> bool {
    // FIX: removed bare contains('/') — too greedy, treats "client/server" as a path.
    // Now only matches tokens that look like actual filesystem paths.
    token == "."
        || token == ".."
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('/')
        || token.starts_with("~/")
}

#[cfg(test)]
mod rgai_arg_tests {
    use super::*;

    #[test]
    fn capability_batch_preserves_order_cardinality_and_subcommand_semantics() {
        let request = serde_json::json!({
            "commands": [
                "bun test",
                "git status --short",
                "ssh host remote-command",
                "gh pr list",
                "cargo test --workspace",
                "cargo publish",
                "git config --list",
                "unknown-tool secret=value"
            ]
        });
        let mut output = Vec::new();

        run_capability_batch(request.to_string().as_bytes(), &mut output)
            .expect("capability batch");
        let response: serde_json::Value =
            serde_json::from_slice(&output).expect("capability response");

        assert_eq!(
            response["supported"],
            serde_json::json!([true, true, true, true, true, false, false, false])
        );
    }

    #[test]
    fn capability_batch_rejects_malformed_or_untyped_stdin() {
        assert!(run_capability_batch(b"{not-json".as_slice(), Vec::new()).is_err());
        assert!(run_capability_batch(
            br#"{"commands":[],"payload":"must-not-be-accepted"}"#.as_slice(),
            Vec::new()
        )
        .is_err());
    }

    #[test]
    fn normalize_rgai_keeps_multiword_query() {
        let (query, path) = normalize_rgai_args(
            vec!["token".to_string(), "refresh".to_string()],
            ".".to_string(),
        );
        assert_eq!(query, "token refresh");
        assert_eq!(path, ".");
    }

    #[test]
    fn normalize_rgai_supports_old_positional_path() {
        let (query, path) = normalize_rgai_args(
            vec!["auth".to_string(), "flow".to_string(), "./src".to_string()],
            ".".to_string(),
        );
        assert_eq!(query, "auth flow");
        assert_eq!(path, "./src");
    }

    #[test]
    fn normalize_rgai_does_not_treat_plain_word_as_path() {
        let (query, path) = normalize_rgai_args(
            vec!["domain".to_string(), "model".to_string()],
            ".".to_string(),
        );
        assert_eq!(query, "domain model");
        assert_eq!(path, ".");
    }

    // FIX: slash-containing words like "client/server" must NOT be treated as paths
    #[test]
    fn normalize_rgai_does_not_treat_slash_word_as_path() {
        let (query, path) = normalize_rgai_args(
            vec!["client/server".to_string(), "architecture".to_string()],
            ".".to_string(),
        );
        assert_eq!(query, "client/server architecture");
        assert_eq!(path, ".");
    }

    #[test]
    fn looks_like_path_recognizes_real_paths() {
        assert!(looks_like_path_token("./src"));
        assert!(looks_like_path_token("../lib"));
        assert!(looks_like_path_token("/usr/local"));
        assert!(looks_like_path_token("~/projects"));
        assert!(looks_like_path_token("."));
        assert!(looks_like_path_token(".."));
    }

    #[test]
    fn looks_like_path_rejects_non_paths() {
        assert!(!looks_like_path_token("client/server"));
        assert!(!looks_like_path_token("input/output"));
        assert!(!looks_like_path_token("read/write"));
    }

    // fix #200: try_parse fallback tests
    #[test]
    fn test_try_parse_valid_git_status() {
        let result = Cli::try_parse_from(["rtk", "git", "status"]);
        assert!(result.is_ok(), "git status should parse successfully");
    }

    #[test]
    fn test_try_parse_help_is_display_help() {
        match Cli::try_parse_from(["rtk", "--help"]) {
            Err(e) => assert_eq!(e.kind(), ErrorKind::DisplayHelp),
            Ok(_) => panic!("Expected DisplayHelp error"),
        }
    }

    #[test]
    fn test_try_parse_version_is_display_version() {
        match Cli::try_parse_from(["rtk", "--version"]) {
            Err(e) => assert_eq!(e.kind(), ErrorKind::DisplayVersion),
            Ok(_) => panic!("Expected DisplayVersion error"),
        }
    }

    #[test]
    fn test_try_parse_unknown_subcommand_is_error() {
        match Cli::try_parse_from(["rtk", "nonexistent-command"]) {
            Err(e) => assert!(!matches!(
                e.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            )),
            Ok(_) => panic!("Expected parse error for unknown subcommand"),
        }
    }

    #[test]
    fn test_try_parse_oc_get() {
        let cli = Cli::try_parse_from(["rtk", "oc", "get", "pods", "-n", "default"]).unwrap();

        match cli.command {
            Commands::Oc {
                command: OcCommands::Get { args },
            } => assert_eq!(args, vec!["pods", "-n", "default"]),
            _ => panic!("Expected Oc Get command"),
        }
    }

    #[test]
    fn test_try_parse_oc_other() {
        let cli = Cli::try_parse_from(["rtk", "oc", "new-project", "test"]).unwrap();

        assert!(matches!(
            cli.command,
            Commands::Oc {
                command: OcCommands::Other(_)
            }
        ));
    }

    #[test]
    fn test_gain_failures_flag_parses() {
        let result = Cli::try_parse_from(["rtk", "gain", "--failures"]);
        assert!(result.is_ok(), "gain --failures should parse");
        if let Ok(cli) = result {
            match cli.command {
                Commands::Gain { failures, .. } => assert!(failures),
                _ => panic!("Expected Gain command"),
            }
        }
    }

    // fix #268: proxy streaming — Clap-level parsing tests

    #[test]
    fn test_proxy_clap_parses() {
        let result = Cli::try_parse_from(["rtk", "proxy", "echo", "hello"]);
        assert!(result.is_ok(), "rtk proxy echo hello should parse");
    }

    #[test]
    fn test_raw_alias_clap_parses_as_proxy() {
        let cli = Cli::try_parse_from(["rtk", "raw", "grep", "needle", "."])
            .expect("rtk raw should parse");
        assert!(matches!(cli.command, Commands::Proxy { .. }));
    }

    // fix #192: git global options — Clap-level parsing tests

    #[test]
    fn test_git_no_pager_parses() {
        let result = Cli::try_parse_from(["rtk", "git", "--no-pager", "log", "--oneline"]);
        assert!(
            result.is_ok(),
            "rtk git --no-pager log should parse: {:?}",
            result.err()
        );
        if let Ok(cli) = result {
            match cli.command {
                Commands::Git { no_pager, .. } => assert!(no_pager, "--no-pager should be true"),
                _ => panic!("Expected Git command"),
            }
        }
    }

    #[test]
    fn test_git_capital_c_parses() {
        let result = Cli::try_parse_from(["rtk", "git", "-C", "/tmp", "status"]);
        assert!(
            result.is_ok(),
            "rtk git -C /tmp status should parse: {:?}",
            result.err()
        );
        if let Ok(cli) = result {
            match cli.command {
                Commands::Git { directory, .. } => {
                    assert_eq!(directory, vec!["/tmp".to_string()]);
                }
                _ => panic!("Expected Git command"),
            }
        }
    }

    #[test]
    fn test_git_no_optional_locks_parses() {
        let result =
            Cli::try_parse_from(["rtk", "git", "--no-pager", "--no-optional-locks", "status"]);
        assert!(
            result.is_ok(),
            "combined global flags should parse: {:?}",
            result.err()
        );
        if let Ok(cli) = result {
            match cli.command {
                Commands::Git {
                    no_pager,
                    no_optional_locks,
                    ..
                } => {
                    assert!(no_pager);
                    assert!(no_optional_locks);
                }
                _ => panic!("Expected Git command"),
            }
        }
    }

    #[test]
    fn test_git_git_dir_parses() {
        let result = Cli::try_parse_from(["rtk", "git", "--git-dir=/tmp/.git", "status"]);
        assert!(
            result.is_ok(),
            "git --git-dir should parse: {:?}",
            result.err()
        );
        if let Ok(cli) = result {
            match cli.command {
                Commands::Git { git_dir, .. } => {
                    assert_eq!(git_dir, Some("/tmp/.git".to_string()));
                }
                _ => panic!("Expected Git command"),
            }
        }
    }
}
