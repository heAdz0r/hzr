//! Token savings tracking and analytics system.
//!
//! This module provides comprehensive tracking of RTK command executions,
//! recording token savings, execution times, and providing aggregation APIs
//! for daily/weekly/monthly statistics.
//!
//! # Architecture
//!
//! - Storage: SQLite database (~/.local/share/rtk/tracking.db)
//! - Retention: 90-day automatic cleanup
//! - Metrics: Input/output tokens, savings %, execution time
//!
//! # Quick Start
//!
//! ```no_run
//! use rtk::tracking::{TimedExecution, Tracker};
//!
//! // Track a command execution
//! let timer = TimedExecution::start();
//! let input = "raw output";
//! let output = "filtered output";
//! timer.track("ls -la", "rtk ls", input, output);
//!
//! // Query statistics
//! let tracker = Tracker::new().unwrap();
//! let summary = tracker.get_summary().unwrap();
//! println!("Saved {} tokens", summary.total_saved);
//! ```
//!
//! See [docs/tracking.md](../docs/tracking.md) for full documentation.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::cell::RefCell;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock; // H4: project_path cache
use std::time::Instant;

// ── Project path helpers ── // added: project-scoped tracking support

/// Get the canonical project path string for the current working directory.
/// H4: cached — CWD is fixed for the lifetime of the rtk process (2 syscalls → 0).
fn current_project_path_string() -> &'static str {
    static PROJECT_PATH: OnceLock<String> = OnceLock::new();
    PROJECT_PATH
        .get_or_init(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.canonicalize().ok())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .as_str()
}

/// Build SQL filter params for project-scoped queries.
/// Returns (exact_match, glob_prefix) for WHERE clause.
/// Uses GLOB instead of LIKE to avoid `_` and `%` in paths acting as wildcards. // changed: GLOB
fn project_filter_params(project_path: Option<&str>) -> (Option<String>, Option<String>) {
    match project_path {
        Some(p) => (
            Some(p.to_string()),
            Some(format!("{}{}*", p, std::path::MAIN_SEPARATOR)), // changed: GLOB pattern with * wildcard
        ),
        None => (None, None),
    }
}

/// CR-08: возвращает WHERE-фрагмент для вставки в SQL-строку.
/// Два отдельных SQL-литерала позволяют SQLite кешировать и оптимизировать
/// их независимо: global — полный scan без фильтра, scoped — index seek по project_path.
fn project_where_clause(scoped: bool) -> &'static str {
    if scoped {
        " WHERE (project_path = ?1 OR project_path GLOB ?2)"
    } else {
        ""
    }
}

/// Number of days to retain tracking history before automatic cleanup.
const HISTORY_DAYS: i64 = 90;
/// Minimum interval between cleanup runs to reduce write amplification.
const CLEANUP_INTERVAL_SECS: i64 = 60 * 60; // 1 hour

thread_local! {
    /// Process-local tracker cache to avoid reopening/migrating SQLite for every track call.
    static TRACKER_CACHE: RefCell<Option<Tracker>> = const { RefCell::new(None) };
}

/// Main tracking interface for recording and querying command history.
///
/// Manages SQLite database connection and provides methods for:
/// - Recording command executions with token counts and timing
/// - Querying aggregated statistics (summary, daily, weekly, monthly)
/// - Retrieving recent command history
///
/// # Database Location
///
/// - Linux: `~/.local/share/rtk/tracking.db`
/// - macOS: `~/Library/Application Support/rtk/tracking.db`
/// - Windows: `%APPDATA%\rtk\tracking.db`
///
/// # Examples
///
/// ```no_run
/// use rtk::tracking::Tracker;
///
/// let tracker = Tracker::new()?;
/// tracker.record("ls -la", "rtk ls", 1000, 200, 50)?;
///
/// let summary = tracker.get_summary()?;
/// println!("Total saved: {} tokens", summary.total_saved);
/// # Ok::<(), anyhow::Error>(())
/// ```
pub struct Tracker {
    conn: Connection,
}

/// Individual command record from tracking history.
///
/// Contains timestamp, command name, and savings metrics for a single execution.
#[derive(Debug)]
pub struct CommandRecord {
    /// UTC timestamp when command was executed
    pub timestamp: DateTime<Utc>,
    /// RTK command that was executed (e.g., "rtk ls")
    pub rtk_cmd: String,
    /// Number of tokens saved (input - output)
    pub saved_tokens: usize,
    /// Savings percentage ((saved / input) * 100)
    pub savings_pct: f64,
}

/// Aggregated statistics across all recorded commands.
///
/// Provides overall metrics and breakdowns by command and by day.
/// Returned by [`Tracker::get_summary`].
#[derive(Debug)]
pub struct GainSummary {
    /// Total number of commands recorded
    pub total_commands: usize,
    /// Total input tokens across all commands
    pub total_input: usize,
    /// Total output tokens across all commands
    pub total_output: usize,
    /// Total tokens saved (input - output)
    pub total_saved: usize,
    /// Average savings percentage across all commands
    pub avg_savings_pct: f64,
    /// Total execution time across all commands (milliseconds)
    pub total_time_ms: u64,
    /// Average execution time per command (milliseconds)
    pub avg_time_ms: u64,
    /// Top 10 commands by tokens saved: (cmd, count, saved, avg_pct, avg_time_ms)
    pub by_command: Vec<(String, usize, usize, f64, u64)>,
    /// Last 30 days of activity: (date, saved_tokens)
    pub by_day: Vec<(String, usize)>,
}

/// Daily statistics for token savings and execution metrics.
///
/// Serializable to JSON for export via `rtk gain --daily --format json`.
///
/// # JSON Schema
///
/// ```json
/// {
///   "date": "2026-02-03",
///   "commands": 42,
///   "input_tokens": 15420,
///   "output_tokens": 3842,
///   "saved_tokens": 11578,
///   "savings_pct": 75.08,
///   "total_time_ms": 8450,
///   "avg_time_ms": 201
/// }
/// ```
#[derive(Debug, Serialize)]
pub struct DayStats {
    /// ISO date (YYYY-MM-DD)
    pub date: String,
    /// Number of commands executed this day
    pub commands: usize,
    /// Total input tokens for this day
    pub input_tokens: usize,
    /// Total output tokens for this day
    pub output_tokens: usize,
    /// Total tokens saved this day
    pub saved_tokens: usize,
    /// Savings percentage for this day
    pub savings_pct: f64,
    /// Total execution time for this day (milliseconds)
    pub total_time_ms: u64,
    /// Average execution time per command (milliseconds)
    pub avg_time_ms: u64,
}

/// Weekly statistics for token savings and execution metrics.
///
/// Serializable to JSON for export via `rtk gain --weekly --format json`.
/// Weeks start on Sunday (SQLite default).
#[derive(Debug, Serialize)]
pub struct WeekStats {
    /// Week start date (YYYY-MM-DD)
    pub week_start: String,
    /// Week end date (YYYY-MM-DD)
    pub week_end: String,
    /// Number of commands executed this week
    pub commands: usize,
    /// Total input tokens for this week
    pub input_tokens: usize,
    /// Total output tokens for this week
    pub output_tokens: usize,
    /// Total tokens saved this week
    pub saved_tokens: usize,
    /// Savings percentage for this week
    pub savings_pct: f64,
    /// Total execution time for this week (milliseconds)
    pub total_time_ms: u64,
    /// Average execution time per command (milliseconds)
    pub avg_time_ms: u64,
}

/// Monthly statistics for token savings and execution metrics.
///
/// Serializable to JSON for export via `rtk gain --monthly --format json`.
#[derive(Debug, Serialize)]
pub struct MonthStats {
    /// Month identifier (YYYY-MM)
    pub month: String,
    /// Number of commands executed this month
    pub commands: usize,
    /// Total input tokens for this month
    pub input_tokens: usize,
    /// Total output tokens for this month
    pub output_tokens: usize,
    /// Total tokens saved this month
    pub saved_tokens: usize,
    /// Savings percentage for this month
    pub savings_pct: f64,
    /// Total execution time for this month (milliseconds)
    pub total_time_ms: u64,
    /// Average execution time per command (milliseconds)
    pub avg_time_ms: u64,
}

impl Tracker {
    /// Create a new tracker instance.
    ///
    /// Opens or creates the SQLite database at the platform-specific location.
    /// Automatically creates the `commands` table if it doesn't exist and runs
    /// any necessary schema migrations.
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Cannot determine database path
    /// - Cannot create parent directories
    /// - Cannot open/create SQLite database
    /// - Schema creation/migration fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn new() -> Result<Self> {
        let db_path = get_db_path()?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&db_path)?;
        configure_connection(&conn);
        conn.execute(
            "CREATE TABLE IF NOT EXISTS commands (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                original_cmd TEXT NOT NULL,
                rtk_cmd TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                saved_tokens INTEGER NOT NULL,
                savings_pct REAL NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_timestamp ON commands(timestamp)",
            [],
        )?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS tracking_meta (
                key TEXT PRIMARY KEY,
                value INTEGER NOT NULL
            )",
            [],
        )?;

        // Migration: add exec_time_ms column if it doesn't exist
        let _ = conn.execute(
            "ALTER TABLE commands ADD COLUMN exec_time_ms INTEGER DEFAULT 0",
            [],
        );
        // Migration: add project_path column with DEFAULT '' for new rows // changed: added DEFAULT
        let _ = conn.execute(
            "ALTER TABLE commands ADD COLUMN project_path TEXT DEFAULT ''",
            [],
        );
        // One-time migration: normalize NULLs from pre-default schema // changed: guarded with EXISTS
        let has_nulls: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM commands WHERE project_path IS NULL)",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if has_nulls {
            let _ = conn.execute(
                "UPDATE commands SET project_path = '' WHERE project_path IS NULL",
                [],
            );
        }
        // Index for fast project-scoped gain queries // added
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_project_path_timestamp ON commands(project_path, timestamp)",
            [],
        );

        // fix #200: parse_failures table for fallback analytics
        conn.execute(
            "CREATE TABLE IF NOT EXISTS parse_failures (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                raw_command TEXT NOT NULL,
                error_message TEXT NOT NULL,
                fallback_succeeded INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_pf_timestamp ON parse_failures(timestamp)",
            [],
        )?;

        Ok(Self { conn })
    }

    /// Record a command execution with token counts and timing.
    ///
    /// Calculates savings metrics and stores the record in the database.
    /// Automatically cleans up records older than 90 days after insertion.
    ///
    /// # Arguments
    ///
    /// - `original_cmd`: The standard command (e.g., "ls -la")
    /// - `rtk_cmd`: The RTK command used (e.g., "rtk ls")
    /// - `input_tokens`: Estimated tokens from standard command output
    /// - `output_tokens`: Actual tokens from RTK output
    /// - `exec_time_ms`: Execution time in milliseconds
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// tracker.record("ls -la", "rtk ls", 1000, 200, 50)?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn record(
        &self,
        original_cmd: &str,
        rtk_cmd: &str,
        input_tokens: usize,
        output_tokens: usize,
        exec_time_ms: u64,
    ) -> Result<()> {
        let saved = input_tokens.saturating_sub(output_tokens);
        let pct = if input_tokens > 0 {
            (saved as f64 / input_tokens as f64) * 100.0
        } else {
            0.0
        };

        let project_path = current_project_path_string(); // added: record cwd

        self.conn.execute(
            "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, project_path, input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)", // added: project_path
            params![
                Utc::now().to_rfc3339(),
                original_cmd,
                rtk_cmd,
                project_path, // added
                input_tokens as i64,
                output_tokens as i64,
                saved as i64,
                pct,
                exec_time_ms as i64
            ],
        )?;

        self.maybe_cleanup_old()?;
        Ok(())
    }

    fn maybe_cleanup_old(&self) -> Result<()> {
        let now = Utc::now().timestamp();
        let last_cleanup: Option<i64> = self
            .conn
            .query_row(
                "SELECT value FROM tracking_meta WHERE key = 'last_cleanup_ts'",
                [],
                |row| row.get(0),
            )
            .optional()?;

        if last_cleanup
            .map(|ts| now.saturating_sub(ts) < CLEANUP_INTERVAL_SECS)
            .unwrap_or(false)
        {
            return Ok(());
        }

        self.cleanup_old()?;
        self.conn.execute(
            "INSERT INTO tracking_meta (key, value)
             VALUES ('last_cleanup_ts', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![now],
        )?;
        Ok(())
    }

    fn cleanup_old(&self) -> Result<()> {
        let cutoff = Utc::now() - chrono::Duration::days(HISTORY_DAYS);
        self.conn.execute(
            "DELETE FROM commands WHERE timestamp < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        // fix #200: also clean up old parse failures
        self.conn.execute(
            "DELETE FROM parse_failures WHERE timestamp < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        Ok(())
    }

    /// Get overall summary statistics across all recorded commands.
    ///
    /// Returns aggregated metrics including:
    /// - Total commands, tokens (input/output/saved)
    /// - Average savings percentage and execution time
    /// - Top 10 commands by tokens saved
    /// - Last 30 days of activity
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// let summary = tracker.get_summary()?;
    /// println!("Saved {} tokens ({:.1}%)",
    ///     summary.total_saved, summary.avg_savings_pct);
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn get_summary(&self) -> Result<GainSummary> {
        self.get_summary_filtered(None) // delegate to filtered variant
    }

    /// Get summary statistics filtered by project path. // added
    ///
    /// When `project_path` is `Some`, matches the exact working directory
    /// or any subdirectory (prefix match with path separator).
    pub fn get_summary_filtered(&self, project_path: Option<&str>) -> Result<GainSummary> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added

        // CR-01 + CR-08: одна агрегирующая SQL-строка; два отдельных SQL-литерала
        // позволяют SQLite кешировать global и scoped варианты с разными планами выполнения.
        let agg_mapper = |row: &rusqlite::Row<'_>| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, i64>(1)? as usize,
                row.get::<_, i64>(2)? as usize,
                row.get::<_, i64>(3)? as usize,
                row.get::<_, i64>(4)? as u64,
            ))
        };
        let (total_commands, total_input, total_output, total_saved, total_time_ms): (
            usize,
            usize,
            usize,
            usize,
            u64,
        ) = match (project_exact.as_deref(), project_glob.as_deref()) {
            (Some(exact), Some(glob)) => self.conn.query_row(
                "SELECT COUNT(*), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                        COALESCE(SUM(saved_tokens),0), COALESCE(SUM(exec_time_ms),0)
                 FROM commands WHERE (project_path = ?1 OR project_path GLOB ?2)", // CR-08: index-friendly
                params![exact, glob],
                agg_mapper,
            )?,
            _ => self.conn.query_row(
                "SELECT COUNT(*), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                        COALESCE(SUM(saved_tokens),0), COALESCE(SUM(exec_time_ms),0)
                 FROM commands", // CR-08: global — чистый full scan без nullable OR
                [],
                agg_mapper,
            )?,
        };

        let avg_savings_pct = if total_input > 0 {
            (total_saved as f64 / total_input as f64) * 100.0
        } else {
            0.0
        };

        let avg_time_ms = if total_commands > 0 {
            total_time_ms / total_commands as u64
        } else {
            0
        };

        // CR-03: BEGIN DEFERRED гарантирует что get_by_command + get_by_day видят
        // один snapshot и разделяют горячий B-tree page cache — без повторного I/O.
        // rayon::join невозможен (Connection не Sync); read tx — минимальный overhead.
        self.conn.execute_batch("BEGIN DEFERRED")?;
        let by_command = self.get_by_command(project_path);
        let by_day = self.get_by_day(project_path);
        let _ = self.conn.execute_batch("COMMIT"); // readonly tx — всегда успешен
        let by_command = by_command?; // added: pass project filter
        let by_day = by_day?; // added: pass project filter

        Ok(GainSummary {
            total_commands,
            total_input,
            total_output,
            total_saved,
            avg_savings_pct,
            total_time_ms,
            avg_time_ms,
            by_command,
            by_day,
        })
    }

    fn get_by_command(
        &self,
        project_path: Option<&str>, // added
    ) -> Result<Vec<(String, usize, usize, f64, u64)>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let cmd_mapper = |row: &rusqlite::Row<'_>| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? as usize,
                row.get::<_, i64>(2)? as usize,
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)? as u64,
            ))
        };
        // CR-08: два отдельных SQL-текста → SQLite кеширует и оптимизирует независимо
        let rows: Vec<_> = match (project_exact.as_deref(), project_glob.as_deref()) {
            (Some(exact), Some(glob)) => self
                .conn
                .prepare_cached(
                    "SELECT rtk_cmd, COUNT(*), SUM(saved_tokens), AVG(savings_pct), AVG(exec_time_ms)
                     FROM commands WHERE (project_path = ?1 OR project_path GLOB ?2)
                     GROUP BY rtk_cmd ORDER BY SUM(saved_tokens) DESC LIMIT 10",
                )?
                .query_map(params![exact, glob], cmd_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
            _ => self
                .conn
                .prepare_cached(
                    "SELECT rtk_cmd, COUNT(*), SUM(saved_tokens), AVG(savings_pct), AVG(exec_time_ms)
                     FROM commands
                     GROUP BY rtk_cmd ORDER BY SUM(saved_tokens) DESC LIMIT 10",
                )?
                .query_map([], cmd_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    fn get_by_day(
        &self,
        project_path: Option<&str>, // added
    ) -> Result<Vec<(String, usize)>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let day_mapper = |row: &rusqlite::Row<'_>| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        };
        // CR-08: два отдельных SQL-текста → SQLite кеширует и оптимизирует независимо
        let mut result: Vec<_> = match (project_exact.as_deref(), project_glob.as_deref()) {
            (Some(exact), Some(glob)) => self
                .conn
                .prepare_cached(
                    "SELECT DATE(timestamp), SUM(saved_tokens) FROM commands
                     WHERE (project_path = ?1 OR project_path GLOB ?2)
                     GROUP BY DATE(timestamp) ORDER BY DATE(timestamp) DESC LIMIT 30",
                )?
                .query_map(params![exact, glob], day_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
            _ => self
                .conn
                .prepare_cached(
                    "SELECT DATE(timestamp), SUM(saved_tokens) FROM commands
                     GROUP BY DATE(timestamp) ORDER BY DATE(timestamp) DESC LIMIT 30",
                )?
                .query_map([], day_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        result.reverse();
        Ok(result)
    }

    /// Get daily statistics for all recorded days.
    ///
    /// Returns one [`DayStats`] per day with commands executed, tokens saved,
    /// and execution time metrics. Results are ordered chronologically (oldest first).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// let days = tracker.get_all_days()?;
    /// for day in days.iter().take(7) {
    ///     println!("{}: {} commands, {} tokens saved",
    ///         day.date, day.commands, day.saved_tokens);
    /// }
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn get_all_days(&self) -> Result<Vec<DayStats>> {
        self.get_all_days_filtered(None) // delegate to filtered variant
    }

    /// Get daily statistics filtered by project path. // added
    pub fn get_all_days_filtered(&self, project_path: Option<&str>) -> Result<Vec<DayStats>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let day_stats_mapper = |row: &rusqlite::Row<'_>| {
            let input = row.get::<_, i64>(2)? as usize;
            let saved = row.get::<_, i64>(4)? as usize;
            let commands = row.get::<_, i64>(1)? as usize;
            let total_time = row.get::<_, i64>(5)? as u64;
            let savings_pct = if input > 0 {
                (saved as f64 / input as f64) * 100.0
            } else {
                0.0
            };
            let avg_time_ms = if commands > 0 {
                total_time / commands as u64
            } else {
                0
            };
            Ok(DayStats {
                date: row.get(0)?,
                commands,
                input_tokens: input,
                output_tokens: row.get::<_, i64>(3)? as usize,
                saved_tokens: saved,
                savings_pct,
                total_time_ms: total_time,
                avg_time_ms,
            })
        };
        // CR-08: два SQL-литерала — SQLite оптимизирует global и scoped независимо
        let mut result: Vec<_> = match (project_exact.as_deref(), project_glob.as_deref()) {
            (Some(exact), Some(glob)) => self
                .conn
                .prepare_cached(
                    "SELECT DATE(timestamp) as date, COUNT(*) as commands,
                        SUM(input_tokens) as input, SUM(output_tokens) as output,
                        SUM(saved_tokens) as saved, SUM(exec_time_ms) as total_time
                 FROM commands WHERE (project_path = ?1 OR project_path GLOB ?2)
                 GROUP BY DATE(timestamp) ORDER BY DATE(timestamp) DESC",
                )?
                .query_map(params![exact, glob], day_stats_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
            _ => self
                .conn
                .prepare_cached(
                    "SELECT DATE(timestamp) as date, COUNT(*) as commands,
                        SUM(input_tokens) as input, SUM(output_tokens) as output,
                        SUM(saved_tokens) as saved, SUM(exec_time_ms) as total_time
                 FROM commands
                 GROUP BY DATE(timestamp) ORDER BY DATE(timestamp) DESC",
                )?
                .query_map([], day_stats_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        result.reverse();
        Ok(result)
    }

    /// Get weekly statistics grouped by week.
    ///
    /// Returns one [`WeekStats`] per week with aggregated metrics.
    /// Weeks start on Sunday (SQLite default). Results ordered chronologically.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// let weeks = tracker.get_by_week()?;
    /// for week in weeks {
    ///     println!("{} to {}: {} tokens saved",
    ///         week.week_start, week.week_end, week.saved_tokens);
    /// }
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn get_by_week(&self) -> Result<Vec<WeekStats>> {
        self.get_by_week_filtered(None) // delegate to filtered variant
    }

    /// Get weekly statistics filtered by project path. // added
    pub fn get_by_week_filtered(&self, project_path: Option<&str>) -> Result<Vec<WeekStats>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let week_mapper = |row: &rusqlite::Row<'_>| {
            let input = row.get::<_, i64>(3)? as usize;
            let saved = row.get::<_, i64>(5)? as usize;
            let commands = row.get::<_, i64>(2)? as usize;
            let total_time = row.get::<_, i64>(6)? as u64;
            let savings_pct = if input > 0 {
                (saved as f64 / input as f64) * 100.0
            } else {
                0.0
            };
            let avg_time_ms = if commands > 0 {
                total_time / commands as u64
            } else {
                0
            };
            Ok(WeekStats {
                week_start: row.get(0)?,
                week_end: row.get(1)?,
                commands,
                input_tokens: input,
                output_tokens: row.get::<_, i64>(4)? as usize,
                saved_tokens: saved,
                savings_pct,
                total_time_ms: total_time,
                avg_time_ms,
            })
        };
        // CR-08: два SQL-литерала — SQLite оптимизирует global и scoped независимо
        let mut result: Vec<_> = match (project_exact.as_deref(), project_glob.as_deref()) {
            (Some(exact), Some(glob)) => self
                .conn
                .prepare_cached(
                    "SELECT DATE(timestamp,'weekday 0','-6 days') as week_start,
                        DATE(timestamp,'weekday 0') as week_end, COUNT(*) as commands,
                        SUM(input_tokens) as input, SUM(output_tokens) as output,
                        SUM(saved_tokens) as saved, SUM(exec_time_ms) as total_time
                 FROM commands WHERE (project_path = ?1 OR project_path GLOB ?2)
                 GROUP BY week_start ORDER BY week_start DESC",
                )?
                .query_map(params![exact, glob], week_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
            _ => self
                .conn
                .prepare_cached(
                    "SELECT DATE(timestamp,'weekday 0','-6 days') as week_start,
                        DATE(timestamp,'weekday 0') as week_end, COUNT(*) as commands,
                        SUM(input_tokens) as input, SUM(output_tokens) as output,
                        SUM(saved_tokens) as saved, SUM(exec_time_ms) as total_time
                 FROM commands
                 GROUP BY week_start ORDER BY week_start DESC",
                )?
                .query_map([], week_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        result.reverse();
        Ok(result)
    }

    /// Get monthly statistics grouped by month.
    ///
    /// Returns one [`MonthStats`] per month (YYYY-MM format) with aggregated metrics.
    /// Results ordered chronologically.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// let months = tracker.get_by_month()?;
    /// for month in months {
    ///     println!("{}: {} tokens saved ({:.1}%)",
    ///         month.month, month.saved_tokens, month.savings_pct);
    /// }
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn get_by_month(&self) -> Result<Vec<MonthStats>> {
        self.get_by_month_filtered(None) // delegate to filtered variant
    }

    /// Get monthly statistics filtered by project path. // added
    pub fn get_by_month_filtered(&self, project_path: Option<&str>) -> Result<Vec<MonthStats>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let month_mapper = |row: &rusqlite::Row<'_>| {
            let input = row.get::<_, i64>(2)? as usize;
            let saved = row.get::<_, i64>(4)? as usize;
            let commands = row.get::<_, i64>(1)? as usize;
            let total_time = row.get::<_, i64>(5)? as u64;
            let savings_pct = if input > 0 {
                (saved as f64 / input as f64) * 100.0
            } else {
                0.0
            };
            let avg_time_ms = if commands > 0 {
                total_time / commands as u64
            } else {
                0
            };
            Ok(MonthStats {
                month: row.get(0)?,
                commands,
                input_tokens: input,
                output_tokens: row.get::<_, i64>(3)? as usize,
                saved_tokens: saved,
                savings_pct,
                total_time_ms: total_time,
                avg_time_ms,
            })
        };
        // CR-08: два SQL-литерала — SQLite оптимизирует global и scoped независимо
        let mut result: Vec<_> = match (project_exact.as_deref(), project_glob.as_deref()) {
            (Some(exact), Some(glob)) => self
                .conn
                .prepare_cached(
                    "SELECT strftime('%Y-%m', timestamp) as month, COUNT(*) as commands,
                        SUM(input_tokens) as input, SUM(output_tokens) as output,
                        SUM(saved_tokens) as saved, SUM(exec_time_ms) as total_time
                 FROM commands WHERE (project_path = ?1 OR project_path GLOB ?2)
                 GROUP BY month ORDER BY month DESC",
                )?
                .query_map(params![exact, glob], month_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
            _ => self
                .conn
                .prepare_cached(
                    "SELECT strftime('%Y-%m', timestamp) as month, COUNT(*) as commands,
                        SUM(input_tokens) as input, SUM(output_tokens) as output,
                        SUM(saved_tokens) as saved, SUM(exec_time_ms) as total_time
                 FROM commands
                 GROUP BY month ORDER BY month DESC",
                )?
                .query_map([], month_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        result.reverse();
        Ok(result)
    }

    /// Get recent command history.
    ///
    /// Returns up to `limit` most recent command records, ordered by timestamp (newest first).
    ///
    /// # Arguments
    ///
    /// - `limit`: Maximum number of records to return
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// let recent = tracker.get_recent(10)?;
    /// for cmd in recent {
    ///     println!("{}: {} saved {:.1}%",
    ///         cmd.timestamp, cmd.rtk_cmd, cmd.savings_pct);
    /// }
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn get_recent(&self, limit: usize) -> Result<Vec<CommandRecord>> {
        self.get_recent_filtered(limit, None) // delegate to filtered variant
    }

    /// Get recent command history filtered by project path. // added
    pub fn get_recent_filtered(
        &self,
        limit: usize,
        project_path: Option<&str>,
    ) -> Result<Vec<CommandRecord>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let rec_mapper = |row: &rusqlite::Row<'_>| {
            Ok(CommandRecord {
                timestamp: DateTime::parse_from_rfc3339(&row.get::<_, String>(0)?)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                rtk_cmd: row.get(1)?,
                saved_tokens: row.get::<_, i64>(2)? as usize,
                savings_pct: row.get(3)?,
            })
        };
        // CR-08: два SQL-литерала — SQLite оптимизирует global и scoped независимо
        let rows: Vec<_> = match (project_exact.as_deref(), project_glob.as_deref()) {
            (Some(exact), Some(glob)) => self
                .conn
                .prepare_cached(
                    "SELECT timestamp, rtk_cmd, saved_tokens, savings_pct FROM commands
                 WHERE (project_path = ?1 OR project_path GLOB ?2)
                 ORDER BY timestamp DESC LIMIT ?3",
                )?
                .query_map(params![exact, glob, limit as i64], rec_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
            _ => self
                .conn
                .prepare_cached(
                    "SELECT timestamp, rtk_cmd, saved_tokens, savings_pct FROM commands
                 ORDER BY timestamp DESC LIMIT ?1",
                )?
                .query_map(params![limit as i64], rec_mapper)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    /// fix #200: Record a parse failure for analytics.
    pub fn record_parse_failure(
        &self,
        raw_command: &str,
        error_message: &str,
        fallback_succeeded: bool,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO parse_failures (timestamp, raw_command, error_message, fallback_succeeded)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                Utc::now().to_rfc3339(),
                raw_command,
                error_message,
                fallback_succeeded as i32,
            ],
        )?;
        self.maybe_cleanup_old()?; // CR-04: rate-limited, не запускает DELETE на каждый вызов
        Ok(())
    }

    /// fix #200: Get parse failure summary for `rtk gain --failures`.
    pub fn get_parse_failure_summary(&self) -> Result<ParseFailureSummary> {
        // CR-05: один scan вместо двух отдельных COUNT(*)
        let (total, succeeded): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(fallback_succeeded), 0) FROM parse_failures",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let recovery_rate = if total > 0 {
            (succeeded as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        let mut stmt = self.conn.prepare_cached(
            "SELECT raw_command, COUNT(*) as cnt
             FROM parse_failures
             GROUP BY raw_command
             ORDER BY cnt DESC
             LIMIT 10",
        )?;
        let top_commands = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut stmt = self.conn.prepare_cached(
            "SELECT timestamp, raw_command, fallback_succeeded
             FROM parse_failures
             ORDER BY timestamp DESC
             LIMIT 10",
        )?;
        let recent = stmt
            .query_map([], |row| {
                Ok(ParseFailureRecord {
                    timestamp: row.get(0)?,
                    raw_command: row.get(1)?,
                    fallback_succeeded: row.get::<_, i32>(2)? != 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(ParseFailureSummary {
            total: total as usize,
            recovery_rate,
            top_commands,
            recent,
        })
    }
}

fn get_db_path() -> Result<PathBuf> {
    // Priority 1: Environment variable RTK_DB_PATH
    if let Ok(custom_path) = std::env::var("RTK_DB_PATH") {
        return Ok(PathBuf::from(custom_path));
    }

    // Priority 2: Configuration file
    if let Ok(config) = crate::config::Config::load() {
        if let Some(db_path) = config.tracking.database_path {
            return Ok(db_path);
        }
    }

    // Priority 3: Default platform-specific location
    let data_dir = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    Ok(data_dir.join("rtk").join("history.db"))
}

/// Estimate token count from text using ~4 chars = 1 token heuristic.
///
/// This is a fast approximation suitable for tracking purposes.
/// For precise counts, integrate with your LLM's tokenizer API.
///
/// # Formula
///
/// `tokens = ceil(chars / 4)`
///
/// # Examples
///
/// ```
/// use rtk::tracking::estimate_tokens;
///
/// assert_eq!(estimate_tokens(""), 0);
/// assert_eq!(estimate_tokens("abcd"), 1);  // 4 chars = 1 token
/// assert_eq!(estimate_tokens("abcde"), 2); // 5 chars = ceil(1.25) = 2
/// assert_eq!(estimate_tokens("hello world"), 3); // 11 chars = ceil(2.75) = 3
/// ```
pub fn estimate_tokens(text: &str) -> usize {
    // ~4 chars per token on average
    (text.len() as f64 / 4.0).ceil() as usize
}

/// Helper struct for timing command execution
/// Helper for timing command execution and tracking results.
///
/// Preferred API for tracking commands. Automatically measures execution time
/// and records token savings. Use instead of the deprecated [`track`] function.
///
/// # Examples
///
/// ```no_run
/// use rtk::tracking::TimedExecution;
///
/// let timer = TimedExecution::start();
/// let input = execute_standard_command()?;
/// let output = execute_rtk_command()?;
/// timer.track("ls -la", "rtk ls", &input, &output);
/// # Ok::<(), anyhow::Error>(())
/// ```
/// fix #200: single parse failure record for analytics display.
pub struct ParseFailureRecord {
    pub timestamp: String,
    pub raw_command: String,
    pub fallback_succeeded: bool,
}

/// fix #200: aggregated summary returned by get_parse_failure_summary().
pub struct ParseFailureSummary {
    pub total: usize,
    pub recovery_rate: f64,
    pub top_commands: Vec<(String, usize)>,
    pub recent: Vec<ParseFailureRecord>,
}

/// fix #200: record a parse failure silently (ignores errors, safe to call from fallback path).
pub fn record_parse_failure_silent(
    raw_command: &str,
    error_message: &str,
    fallback_succeeded: bool,
) {
    if let Ok(tracker) = Tracker::new() {
        let _ = tracker.record_parse_failure(raw_command, error_message, fallback_succeeded);
    }
}

pub struct TimedExecution {
    start: Instant,
}

impl TimedExecution {
    /// Start timing a command execution.
    ///
    /// Creates a new timer that starts measuring elapsed time immediately.
    /// Call [`track`](Self::track) or [`track_passthrough`](Self::track_passthrough)
    /// when the command completes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::TimedExecution;
    ///
    /// let timer = TimedExecution::start();
    /// // ... execute command ...
    /// timer.track("cmd", "rtk cmd", "input", "output");
    /// ```
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Track the command with elapsed time and token counts.
    ///
    /// Records the command execution with:
    /// - Elapsed time since [`start`](Self::start)
    /// - Token counts estimated from input/output strings
    /// - Calculated savings metrics
    ///
    /// # Arguments
    ///
    /// - `original_cmd`: Standard command (e.g., "ls -la")
    /// - `rtk_cmd`: RTK command used (e.g., "rtk ls")
    /// - `input`: Standard command output (for token estimation)
    /// - `output`: RTK command output (for token estimation)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::TimedExecution;
    ///
    /// let timer = TimedExecution::start();
    /// let input = "long output...";
    /// let output = "short output";
    /// timer.track("ls -la", "rtk ls", input, output);
    /// ```
    pub fn track(&self, original_cmd: &str, rtk_cmd: &str, input: &str, output: &str) {
        let elapsed_ms = self.start.elapsed().as_millis() as u64;
        let input_tokens = estimate_tokens(input);
        let output_tokens = estimate_tokens(output);

        with_cached_tracker(|tracker| {
            let _ = tracker.record(
                original_cmd,
                rtk_cmd,
                input_tokens,
                output_tokens,
                elapsed_ms,
            );
        });
    }

    /// Track passthrough commands (timing-only, no token counting).
    ///
    /// For commands that stream output or run interactively where output
    /// cannot be captured. Records execution time but sets tokens to 0
    /// (does not dilute savings statistics).
    ///
    /// # Arguments
    ///
    /// - `original_cmd`: Standard command (e.g., "git tag --list")
    /// - `rtk_cmd`: RTK command used (e.g., "rtk git tag --list")
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::TimedExecution;
    ///
    /// let timer = TimedExecution::start();
    /// // ... execute streaming command ...
    /// timer.track_passthrough("git tag", "rtk git tag");
    /// ```
    pub fn track_passthrough(&self, original_cmd: &str, rtk_cmd: &str) {
        let elapsed_ms = self.start.elapsed().as_millis() as u64;
        // input_tokens=0, output_tokens=0 won't dilute savings statistics
        with_cached_tracker(|tracker| {
            let _ = tracker.record(original_cmd, rtk_cmd, 0, 0, elapsed_ms);
        });
    }
}

fn with_cached_tracker<F>(f: F)
where
    F: FnOnce(&Tracker),
{
    TRACKER_CACHE.with(|slot| {
        let mut cache = slot.borrow_mut();
        if cache.is_none() {
            if let Ok(tracker) = Tracker::new() {
                *cache = Some(tracker);
            } else {
                return;
            }
        }
        if let Some(tracker) = cache.as_ref() {
            f(tracker);
        }
    });
}

fn configure_connection(conn: &Connection) {
    // Best-effort pragmas: never fail command execution if sqlite rejects a setting.
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    let _ = conn.busy_timeout(std::time::Duration::from_millis(2500));
}

/// Format OsString args for tracking display.
///
/// Joins arguments with spaces, converting each to UTF-8 (lossy).
/// Useful for displaying command arguments in tracking records.
///
/// # Examples
///
/// ```
/// use std::ffi::OsString;
/// use rtk::tracking::args_display;
///
/// let args = vec![OsString::from("status"), OsString::from("--short")];
/// assert_eq!(args_display(&args), "status --short");
/// ```
pub fn args_display(args: &[OsString]) -> String {
    args.iter()
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Track a command execution (legacy function, use [`TimedExecution`] for new code).
///
#[cfg(test)]
mod tests {
    use super::*;

    // 1. estimate_tokens — verify ~4 chars/token ratio
    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1); // 4 chars = 1 token
        assert_eq!(estimate_tokens("abcde"), 2); // 5 chars = ceil(1.25) = 2
        assert_eq!(estimate_tokens("a"), 1); // 1 char = ceil(0.25) = 1
        assert_eq!(estimate_tokens("12345678"), 2); // 8 chars = 2 tokens
    }

    // 2. args_display — format OsString vec
    #[test]
    fn test_args_display() {
        let args = vec![OsString::from("status"), OsString::from("--short")];
        assert_eq!(args_display(&args), "status --short");
        assert_eq!(args_display(&[]), "");

        let single = vec![OsString::from("log")];
        assert_eq!(args_display(&single), "log");
    }

    // 3. Tracker::record + get_recent — round-trip DB
    #[test]
    fn test_tracker_record_and_recent() {
        let tracker = Tracker::new().expect("Failed to create tracker");

        // Use unique test identifier to avoid conflicts with other tests
        let test_cmd = format!("rtk git status test_{}", std::process::id());

        tracker
            .record("git status", &test_cmd, 100, 20, 50)
            .expect("Failed to record");

        let recent = tracker.get_recent(10).expect("Failed to get recent");

        // Find our specific test record
        let test_record = recent
            .iter()
            .find(|r| r.rtk_cmd == test_cmd)
            .expect("Test record not found in recent commands");

        assert_eq!(test_record.saved_tokens, 80);
        assert_eq!(test_record.savings_pct, 80.0);
    }

    // 4. track_passthrough doesn't dilute stats (input=0, output=0)
    #[test]
    fn test_track_passthrough_no_dilution() {
        let tracker = Tracker::new().expect("Failed to create tracker");

        // Use unique test identifiers
        let pid = std::process::id();
        let cmd1 = format!("rtk cmd1_test_{}", pid);
        let cmd2 = format!("rtk cmd2_passthrough_test_{}", pid);

        // Record one real command with 80% savings
        tracker
            .record("cmd1", &cmd1, 1000, 200, 10)
            .expect("Failed to record cmd1");

        // Record passthrough (0, 0)
        tracker
            .record("cmd2", &cmd2, 0, 0, 5)
            .expect("Failed to record passthrough");

        // Verify both records exist in recent history
        let recent = tracker.get_recent(20).expect("Failed to get recent");

        let record1 = recent
            .iter()
            .find(|r| r.rtk_cmd == cmd1)
            .expect("cmd1 record not found");
        let record2 = recent
            .iter()
            .find(|r| r.rtk_cmd == cmd2)
            .expect("passthrough record not found");

        // Verify cmd1 has 80% savings
        assert_eq!(record1.saved_tokens, 800);
        assert_eq!(record1.savings_pct, 80.0);

        // Verify passthrough has 0% savings
        assert_eq!(record2.saved_tokens, 0);
        assert_eq!(record2.savings_pct, 0.0);

        // This validates that passthrough (0 input, 0 output) doesn't dilute stats
        // because the savings calculation is correct for both cases
    }

    // 5. TimedExecution::track records with exec_time > 0
    #[test]
    fn test_timed_execution_records_time() {
        let timer = TimedExecution::start();
        std::thread::sleep(std::time::Duration::from_millis(10));
        timer.track("test cmd", "rtk test", "raw input data", "filtered");

        // Verify via DB that record exists
        let tracker = Tracker::new().expect("Failed to create tracker");
        let recent = tracker.get_recent(5).expect("Failed to get recent");
        assert!(recent.iter().any(|r| r.rtk_cmd == "rtk test"));
    }

    // 6. TimedExecution::track_passthrough records with 0 tokens
    #[test]
    fn test_timed_execution_passthrough() {
        let timer = TimedExecution::start();
        timer.track_passthrough("git tag", "rtk git tag (passthrough)");

        let tracker = Tracker::new().expect("Failed to create tracker");
        let recent = tracker.get_recent(5).expect("Failed to get recent");

        let pt = recent
            .iter()
            .find(|r| r.rtk_cmd.contains("passthrough"))
            .expect("Passthrough record not found");

        // savings_pct should be 0 for passthrough
        assert_eq!(pt.savings_pct, 0.0);
        assert_eq!(pt.saved_tokens, 0);
    }

    // 7. get_db_path respects environment variable RTK_DB_PATH
    #[test]
    fn test_custom_db_path_env() {
        use std::env;

        let custom_path = "/tmp/rtk_test_custom.db";
        env::set_var("RTK_DB_PATH", custom_path);

        let db_path = get_db_path().expect("Failed to get db path");
        assert_eq!(db_path, PathBuf::from(custom_path));

        env::remove_var("RTK_DB_PATH");
    }

    // 8. get_db_path falls back to default when no custom config
    #[test]
    fn test_default_db_path() {
        use std::env;

        // Ensure no env var is set
        env::remove_var("RTK_DB_PATH");

        let db_path = get_db_path().expect("Failed to get db path");
        assert!(db_path.ends_with("rtk/history.db"));
    }

    // 9. project_filter_params uses GLOB pattern with * wildcard // added
    #[test]
    fn test_project_filter_params_glob_pattern() {
        let (exact, glob) = project_filter_params(Some("/home/user/project"));
        assert_eq!(exact.unwrap(), "/home/user/project");
        // Must use * (GLOB) not % (LIKE) for subdirectory prefix matching
        let glob_val = glob.unwrap();
        assert!(glob_val.ends_with('*'), "GLOB pattern must end with *");
        assert!(!glob_val.contains('%'), "Must not contain LIKE wildcard %");
        assert_eq!(
            glob_val,
            format!("/home/user/project{}*", std::path::MAIN_SEPARATOR)
        );
    }

    // 10. project_filter_params returns None for None input // added
    #[test]
    fn test_project_filter_params_none() {
        let (exact, glob) = project_filter_params(None);
        assert!(exact.is_none());
        assert!(glob.is_none());
    }

    // 11. GLOB pattern safe with underscores in path names // added
    #[test]
    fn test_project_filter_params_underscore_safe() {
        // In LIKE, _ matches any single char; in GLOB, _ is literal
        let (exact, glob) = project_filter_params(Some("/home/user/my_project"));
        assert_eq!(exact.unwrap(), "/home/user/my_project");
        let glob_val = glob.unwrap();
        // _ must be preserved literally (GLOB treats _ as literal, LIKE does not)
        assert!(glob_val.contains("my_project"));
        assert_eq!(
            glob_val,
            format!("/home/user/my_project{}*", std::path::MAIN_SEPARATOR)
        );
    }
}
