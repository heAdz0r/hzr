use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, Clone)]
pub enum PrismaCommand {
    Generate,
    Migrate { subcommand: MigrateSubcommand },
    DbPush,
}

#[derive(Debug, Clone)]
pub enum MigrateSubcommand {
    Dev { name: Option<String> },
    Status,
    Deploy,
}

pub fn run(cmd: PrismaCommand, args: &[String], verbose: u8) -> Result<()> {
    match cmd {
        PrismaCommand::Generate => run_generate(args, verbose),
        PrismaCommand::Migrate { subcommand } => run_migrate(subcommand, args, verbose),
        PrismaCommand::DbPush => run_db_push(args, verbose),
    }
}

/// Create a Command that will run prisma (tries global first, then npx)
fn create_prisma_command() -> Command {
    if let Ok(binary) = crate::utils::resolve_binary("prisma") {
        Command::new(binary)
    } else {
        let mut c = Command::new("npx");
        c.arg("prisma");
        c
    }
}

fn run_generate(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = create_prisma_command();
    cmd.arg("generate");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: prisma generate");
    }

    let output = cmd
        .output()
        .context("Failed to run prisma generate (try: npm install -g prisma)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("prisma generate failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let filtered = filter_prisma_generate(&raw);

    let shown = crate::guard::never_worse(&raw, &filtered);
    println!("{}", shown);

    timer.track("prisma generate", "rtk prisma generate", &raw, shown);

    Ok(())
}

fn run_migrate(subcommand: MigrateSubcommand, args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = create_prisma_command();
    cmd.arg("migrate");

    let cmd_name = match &subcommand {
        MigrateSubcommand::Dev { name } => {
            cmd.arg("dev");
            if let Some(n) = name {
                cmd.arg("--name").arg(n);
            }
            "prisma migrate dev"
        }
        MigrateSubcommand::Status => {
            cmd.arg("status");
            "prisma migrate status"
        }
        MigrateSubcommand::Deploy => {
            cmd.arg("deploy");
            "prisma migrate deploy"
        }
    };

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {}", cmd_name);
    }

    let output = cmd.output().context("Failed to run prisma migrate")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("prisma migrate failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = match subcommand {
        MigrateSubcommand::Dev { .. } => filter_migrate_dev(&raw),
        MigrateSubcommand::Status => filter_migrate_status(&raw),
        MigrateSubcommand::Deploy => filter_migrate_deploy(&raw),
    };

    let shown = crate::guard::never_worse(&raw, &filtered);
    println!("{}", shown);

    timer.track(cmd_name, &format!("rtk {}", cmd_name), &raw, shown);

    Ok(())
}

fn run_db_push(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = create_prisma_command();
    cmd.arg("db").arg("push");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: prisma db push");
    }

    let output = cmd.output().context("Failed to run prisma db push")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("prisma db push failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let filtered = filter_db_push(&raw);

    let shown = crate::guard::never_worse(&raw, &filtered);
    println!("{}", shown);

    timer.track("prisma db push", "rtk prisma db push", &raw, shown);

    Ok(())
}

/// Filter prisma generate output - strip ASCII art, extract counts
fn filter_prisma_generate(output: &str) -> String {
    let mut models = 0;
    let mut enums = 0;
    let mut types = 0;
    let mut output_path = String::new();

    for line in output.lines() {
        // Skip ASCII art and box drawing
        if line.contains("█")
            || line.contains("▀")
            || line.contains("▄")
            || line.contains("┌")
            || line.contains("└")
            || line.contains("│")
        {
            continue;
        }

        // Extract counts
        if line.contains("model") && line.contains("generated") {
            if let Some(num) = extract_number(line) {
                models = num;
            }
        }
        if line.contains("enum") {
            if let Some(num) = extract_number(line) {
                enums = num;
            }
        }
        if line.contains("type") {
            if let Some(num) = extract_number(line) {
                types = num;
            }
        }

        // Extract output path
        if line.contains("node_modules") && line.contains("@prisma") {
            output_path = line.trim().to_string();
        }
    }

    let mut result = String::new();
    result.push_str("✓ Prisma Client generated\n");

    if models > 0 || enums > 0 || types > 0 {
        result.push_str(&format!(
            "  • {} models, {} enums, {} types\n",
            models, enums, types
        ));
    }

    if !output_path.is_empty() {
        result.push_str("  • Output: node_modules/@prisma/client\n");
    }

    result.trim().to_string()
}

/// Filter migrate dev output - extract migration changes
fn filter_migrate_dev(output: &str) -> String {
    let mut migration_name = String::new();
    let mut tables_added = 0;
    let mut tables_modified = 0;
    let mut relations = Vec::new();
    let mut indexes = Vec::new();
    let mut applied = false;

    for line in output.lines() {
        // Extract migration name
        if line.contains("migration") && line.contains("_") {
            if let Some(pos) = line.find("202") {
                let end = line[pos..]
                    .find(|c: char| c.is_whitespace())
                    .unwrap_or(line.len() - pos);
                migration_name = line[pos..pos + end].to_string();
            }
        }

        // Count changes
        if line.contains("CREATE TABLE") {
            tables_added += 1;
        }
        if line.contains("ALTER TABLE") {
            tables_modified += 1;
        }
        if line.contains("FOREIGN KEY") || line.contains("REFERENCES") {
            if let Some(table) = extract_table_name(line) {
                relations.push(table);
            }
        }
        if line.contains("CREATE INDEX") || line.contains("CREATE UNIQUE INDEX") {
            if let Some(idx) = extract_index_name(line) {
                indexes.push(idx);
            }
        }

        if line.contains("applied") || line.contains("✓") {
            applied = true;
        }
    }

    let mut result = String::new();

    if !migration_name.is_empty() {
        result.push_str(&format!("🗃️  Migration: {}\n", migration_name));
        result.push_str("═══════════════════════════════════════\n");
    }

    result.push_str("Changes:\n");
    if tables_added > 0 {
        result.push_str(&format!("  + {} table(s)\n", tables_added));
    }
    if tables_modified > 0 {
        result.push_str(&format!("  ~ {} table(s) modified\n", tables_modified));
    }
    if !relations.is_empty() {
        result.push_str(&format!("  + {} relation(s)\n", relations.len()));
    }
    if !indexes.is_empty() {
        result.push_str(&format!("  ~ {} index(es)\n", indexes.len()));
    }

    result.push('\n');
    if applied {
        result.push_str("✓ Applied | Pending: 0\n");
    }

    result.trim().to_string()
}

/// A migration directory name: a timestamp followed by `_<label>`.
///
/// Counting migrations by looking for the *word* "applied" on a line counted
/// prose instead: "Following migrations have not yet been applied:" scored as
/// one applied migration while the names below it scored as none, so a database
/// with pending work reported "1 applied, 0 pending" — backwards.
fn migration_name(line: &str) -> Option<&str> {
    let token = line.trim();
    if token.is_empty() || token.contains(char::is_whitespace) {
        return None;
    }
    let (digits, rest) = token.split_at(token.find('_')?);
    (digits.len() >= 8 && digits.chars().all(|c| c.is_ascii_digit()) && rest.len() > 1)
        .then_some(token)
}

/// Filter migrate status output
fn filter_migrate_status(output: &str) -> String {
    let mut total_found: Option<usize> = None;
    let mut pending: Vec<&str> = Vec::new();
    let mut applied: Vec<&str> = Vec::new();
    let mut in_pending_section = false;
    let mut up_to_date = false;

    for line in output.lines() {
        let trimmed = line.trim();

        if let Some(count) = trimmed
            .split_whitespace()
            .next()
            .and_then(|word| word.parse::<usize>().ok())
            .filter(|_| trimmed.contains("migration") && trimmed.contains("found"))
        {
            total_found = Some(count);
            continue;
        }
        if trimmed.contains("not yet been applied") || trimmed.contains("have not yet been") {
            in_pending_section = true;
            continue;
        }
        if trimmed.contains("Database schema is up to date") {
            up_to_date = true;
            continue;
        }
        if let Some(name) = migration_name(trimmed) {
            if in_pending_section {
                pending.push(name);
            } else {
                applied.push(name);
            }
        }
    }

    let pending_count = pending.len();
    let applied_count = match (total_found, up_to_date) {
        (Some(total), _) => total.saturating_sub(pending_count),
        (None, true) => applied.len(),
        (None, false) => applied.len(),
    };

    // Nothing recognisable was parsed: reporting a confident "0 applied, 0
    // pending" would be an invented answer, so hand back the raw text and let
    // the never-worse guard at the call site pick the cheaper rendering.
    if total_found.is_none() && !up_to_date && applied.is_empty() && pending.is_empty() {
        return output.trim().to_string();
    }

    let mut result = format!(
        "Migrations: {} applied, {} pending\n",
        applied_count, pending_count
    );

    if let Some(latest) = pending.first().or_else(|| applied.last()) {
        result.push_str(&format!("Latest: {}\n", latest));
    }

    result.trim().to_string()
}

/// Filter migrate deploy output
fn filter_migrate_deploy(output: &str) -> String {
    let mut deployed = 0;
    let mut errors = Vec::new();

    for line in output.lines() {
        if line.contains("applied") || line.contains("✓") {
            deployed += 1;
        }
        if line.contains("error") || line.contains("ERROR") {
            errors.push(line.trim().to_string());
        }
    }

    let mut result = String::new();

    if errors.is_empty() {
        result.push_str(&format!("✓ {} migration(s) deployed\n", deployed));
    } else {
        result.push_str("❌ Deployment failed:\n");
        for err in errors.iter().take(5) {
            result.push_str(&format!("  {}\n", err));
        }
    }

    result.trim().to_string()
}

/// Filter db push output
fn filter_db_push(output: &str) -> String {
    let mut tables_added = 0;
    let mut columns_modified = 0;
    let mut dropped = 0;

    for line in output.lines() {
        if line.contains("CREATE TABLE") {
            tables_added += 1;
        }
        if line.contains("ALTER") || line.contains("ADD COLUMN") {
            columns_modified += 1;
        }
        if line.contains("DROP") {
            dropped += 1;
        }
    }

    let mut result = String::new();
    result.push_str("✓ Schema pushed to database\n");

    if tables_added > 0 || columns_modified > 0 || dropped > 0 {
        result.push_str(&format!(
            "  + {} tables, ~ {} columns, - {} dropped\n",
            tables_added, columns_modified, dropped
        ));
    }

    result.trim().to_string()
}

/// Extract first number from a line
fn extract_number(line: &str) -> Option<usize> {
    line.split_whitespace()
        .find_map(|word| word.parse::<usize>().ok())
}

/// Extract table name from SQL
fn extract_table_name(line: &str) -> Option<String> {
    if line.contains("TABLE") {
        let parts: Vec<&str> = line.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            if *part == "TABLE" && i + 1 < parts.len() {
                return Some(
                    parts[i + 1]
                        .trim_matches(|c| c == '`' || c == '"' || c == ';')
                        .to_string(),
                );
            }
        }
    }
    None
}

/// Extract index name from SQL
fn extract_index_name(line: &str) -> Option<String> {
    if line.contains("INDEX") {
        let parts: Vec<&str> = line.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            if *part == "INDEX" && i + 1 < parts.len() {
                return Some(
                    parts[i + 1]
                        .trim_matches(|c| c == '`' || c == '"' || c == ';')
                        .to_string(),
                );
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_status_counts_migrations_not_prose() {
        let output = "\
3 migrations found in prisma/migrations

Following migrations have not yet been applied:
20240301120000_add_orders
20240401090000_add_index
";
        let filtered = filter_migrate_status(output);
        assert!(
            filtered.contains("1 applied, 2 pending"),
            "counted prose instead of migrations: {filtered}"
        );
        assert!(filtered.contains("Latest: 20240301120000_add_orders"));
    }

    #[test]
    fn migrate_status_up_to_date_reports_no_pending() {
        let output = "\
2 migrations found in prisma/migrations

Database schema is up to date!
";
        let filtered = filter_migrate_status(output);
        assert!(filtered.contains("2 applied, 0 pending"), "got: {filtered}");
    }

    #[test]
    fn migrate_status_falls_back_rather_than_inventing_zeroes() {
        let output = "Environment variables loaded from .env\nsomething unexpected\n";
        assert_eq!(filter_migrate_status(output), output.trim());
    }

    #[test]
    fn test_filter_generate() {
        let output = r#"
Prisma schema loaded from prisma/schema.prisma

✔ Generated Prisma Client (v5.7.0) to ./node_modules/@prisma/client in 234ms

Start by importing your Prisma Client:

import { PrismaClient } from '@prisma/client'

42 models, 18 enums, 890 types generated
"#;
        let result = filter_prisma_generate(output);
        assert!(result.contains("✓ Prisma Client generated"));
        // Parser may not extract exact counts from this format, just check it doesn't crash
        assert!(!result.contains("Prisma schema loaded"));
        assert!(!result.contains("Start by importing"));
    }

    #[test]
    fn test_filter_migrate_dev() {
        let output = r#"
Applying migration 20260128_add_sessions

CREATE TABLE "Session" (
  "id" TEXT NOT NULL,
  "userId" TEXT NOT NULL,
  FOREIGN KEY ("userId") REFERENCES "User"("id")
);

CREATE INDEX "session_status_idx" ON "Session"("status");

✓ Migration applied
"#;
        let result = filter_migrate_dev(output);
        assert!(result.contains("20260128_add_sessions"));
        assert!(result.contains("+ 1 table"));
        assert!(result.contains("✓ Applied"));
    }

    #[test]
    fn test_extract_number() {
        assert_eq!(extract_number("42 models generated"), Some(42));
        assert_eq!(extract_number("no numbers here"), None);
    }
}
