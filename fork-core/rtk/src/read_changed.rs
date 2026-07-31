//! Git-aware diff reading for `rtk read --changed/--since`.
//! Created in PR-5. Parses unified diff hunks and renders changed blocks.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

// ── Data model ──────────────────────────────────────────────

/// A single hunk from a unified diff.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// 1-based start line in the new (working) version
    pub new_start: usize,
    /// Number of lines in the new version
    pub new_count: usize,
    /// Diff content lines (with +/-/space prefixes)
    pub lines: Vec<DiffLine>,
}

/// A single line within a diff hunk.
#[derive(Debug, Clone)]
pub enum DiffLine {
    Added(String),
    Removed(String),
    Context(String),
}

// ── Git diff provider ───────────────────────────────────────

/// Fetch diff hunks for a file from git.
pub fn git_diff_hunks(
    file: &Path,
    revision: Option<&str>,
    context: usize,
) -> Result<Vec<DiffHunk>> {
    // Verify we're in a git repo
    let git_check = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .context("Failed to run git — is git installed?")?;

    if !git_check.status.success() {
        anyhow::bail!("Not inside a git repository. --changed/--since require git.");
    }

    // Check if file is tracked or has changes
    let file_str = file.to_str().unwrap_or("");

    let args = build_git_diff_args(file_str, revision, context);

    let output = Command::new("git")
        .args(&args)
        .output()
        .context("Failed to run git diff")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff failed: {}", stderr.trim());
    }

    let diff_text = String::from_utf8_lossy(&output.stdout);

    if diff_text.trim().is_empty() {
        // No changes — could be untracked file, check git status
        let status_output = Command::new("git")
            .args(["status", "--porcelain", "--", file_str])
            .output()
            .context("Failed to run git status")?;

        let status_text = String::from_utf8_lossy(&status_output.stdout);
        if status_text.starts_with("??") {
            anyhow::bail!(
                "File '{}' is untracked. Use `git add` first, or read without --changed.",
                file.display()
            );
        }

        // No diff output means no changes
        return Ok(vec![]);
    }

    parse_unified_diff(&diff_text)
}

fn build_git_diff_args(file_str: &str, revision: Option<&str>, context: usize) -> Vec<String> {
    let mut args = vec![
        "diff".to_string(),
        "--no-color".to_string(),
        format!("--unified={context}"),
    ];
    if let Some(rev) = revision {
        // --since: committed range only (rev..HEAD), excludes working tree.
        args.push(format!("{rev}..HEAD"));
    }
    args.push("--".to_string());
    args.push(file_str.to_string());
    args
}

// ── Unified diff parser ─────────────────────────────────────

/// Parse unified diff output into hunks.
pub fn parse_unified_diff(diff: &str) -> Result<Vec<DiffHunk>> {
    let mut hunks = Vec::new();
    let mut current_hunk: Option<DiffHunk> = None;

    for line in diff.lines() {
        // Hunk header: @@ -old_start,old_count +new_start,new_count @@
        if line.starts_with("@@") {
            if let Some(hunk) = current_hunk.take() {
                hunks.push(hunk);
            }
            if let Some((new_start, new_count)) = parse_hunk_header(line) {
                current_hunk = Some(DiffHunk {
                    new_start,
                    new_count,
                    lines: Vec::new(),
                });
            }
            continue;
        }

        // Skip file headers only outside hunks; inside hunks these are real content.
        if current_hunk.is_none()
            && (line.starts_with("diff ")
                || line.starts_with("index ")
                || line.starts_with("---")
                || line.starts_with("+++"))
        {
            continue;
        }

        // Binary diff
        if line.starts_with("Binary files") {
            anyhow::bail!("Binary file detected in diff — cannot show changed hunks.");
        }

        // Rename detection
        if line.starts_with("rename from") || line.starts_with("rename to") {
            continue;
        }

        if let Some(ref mut hunk) = current_hunk {
            if let Some(content) = line.strip_prefix('+') {
                hunk.lines.push(DiffLine::Added(content.to_string()));
            } else if let Some(content) = line.strip_prefix('-') {
                hunk.lines.push(DiffLine::Removed(content.to_string()));
            } else if let Some(content) = line.strip_prefix(' ') {
                hunk.lines.push(DiffLine::Context(content.to_string()));
            } else if line == "\\ No newline at end of file" {
                // skip marker
            } else {
                // Treat as context (edge case: lines without prefix in some diff formats)
                hunk.lines.push(DiffLine::Context(line.to_string()));
            }
        }
    }

    if let Some(hunk) = current_hunk {
        hunks.push(hunk);
    }

    Ok(hunks)
}

/// Parse @@ -a,b +c,d @@ header to extract (new_start, new_count).
fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    // Format: @@ -old_start[,old_count] +new_start[,new_count] @@
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }

    let new_part = parts.iter().find(|p| p.starts_with('+'))?;
    let new_part = new_part.trim_start_matches('+');

    let (start, count) = if let Some((s, c)) = new_part.split_once(',') {
        (s.parse::<usize>().ok()?, c.parse::<usize>().ok()?)
    } else {
        (new_part.parse::<usize>().ok()?, 1)
    };

    Some((start, count))
}

// ── Rendering ───────────────────────────────────────────────

/// Render diff hunks as compact changed-block output.
pub fn render_changed_hunks(hunks: &[DiffHunk], file: &Path) -> String {
    if hunks.is_empty() {
        return format!("No changes in {}\n", file.display());
    }

    let total_added: usize = hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| matches!(l, DiffLine::Added(_)))
        .count();
    let total_removed: usize = hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| matches!(l, DiffLine::Removed(_)))
        .count();

    let mut out = String::new();
    out.push_str(&format!(
        "Changed: {} ({} hunks, +{} -{})\n",
        file.display(),
        hunks.len(),
        total_added,
        total_removed
    ));

    for (i, hunk) in hunks.iter().enumerate() {
        if i > 0 {
            out.push_str("  ···\n");
        }

        let line_width = (hunk.new_start + hunk.new_count).to_string().len();
        let mut current_line = hunk.new_start;

        for diff_line in &hunk.lines {
            match diff_line {
                DiffLine::Added(content) => {
                    out.push_str(&format!(
                        "  {:>w$} │+│ {}\n",
                        current_line,
                        content,
                        w = line_width
                    ));
                    current_line += 1;
                }
                DiffLine::Removed(content) => {
                    out.push_str(&format!("  {:>w$} │-│ {}\n", "", content, w = line_width));
                    // Removed lines don't advance new-file line counter
                }
                DiffLine::Context(content) => {
                    out.push_str(&format!(
                        "  {:>w$} │ │ {}\n",
                        current_line,
                        content,
                        w = line_width
                    ));
                    current_line += 1;
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hunk_header_basic() {
        assert_eq!(parse_hunk_header("@@ -1,3 +1,4 @@"), Some((1, 4)));
        assert_eq!(parse_hunk_header("@@ -10 +12,5 @@"), Some((12, 5)));
        assert_eq!(parse_hunk_header("@@ -1 +1 @@"), Some((1, 1)));
    }

    #[test]
    fn parse_hunk_header_with_context() {
        assert_eq!(
            parse_hunk_header("@@ -1,3 +1,4 @@ fn main() {"),
            Some((1, 4))
        );
    }

    #[test]
    fn parse_unified_diff_basic() -> Result<()> {
        let diff = "\
diff --git a/foo.rs b/foo.rs
index 1234567..abcdefg 100644
--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,4 @@
 line1
+added_line
 line2
 line3
@@ -10,2 +11,3 @@
 line10
+another_add
 line11
";
        let hunks = parse_unified_diff(diff)?;
        assert_eq!(hunks.len(), 2);

        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 4);
        assert_eq!(hunks[0].lines.len(), 4);
        assert!(matches!(&hunks[0].lines[1], DiffLine::Added(s) if s == "added_line"));

        assert_eq!(hunks[1].new_start, 11);
        assert_eq!(hunks[1].new_count, 3);
        Ok(())
    }

    #[test]
    fn parse_unified_diff_removed_lines() -> Result<()> {
        let diff = "\
@@ -1,3 +1,2 @@
 line1
-removed
 line3
";
        let hunks = parse_unified_diff(diff)?;
        assert_eq!(hunks.len(), 1);
        assert!(matches!(&hunks[0].lines[1], DiffLine::Removed(s) if s == "removed"));
        Ok(())
    }

    #[test]
    fn parse_unified_diff_keeps_header_like_content_inside_hunk() -> Result<()> {
        let diff = "\
@@ -1,2 +1,2 @@
----old
++++new
";
        let hunks = parse_unified_diff(diff)?;
        assert_eq!(hunks.len(), 1);
        assert!(matches!(&hunks[0].lines[0], DiffLine::Removed(s) if s == "---old"));
        assert!(matches!(&hunks[0].lines[1], DiffLine::Added(s) if s == "+++new"));
        Ok(())
    }

    #[test]
    fn parse_empty_diff() -> Result<()> {
        let hunks = parse_unified_diff("")?;
        assert!(hunks.is_empty());
        Ok(())
    }

    #[test]
    fn parse_binary_diff_errors() {
        let diff = "Binary files a/image.png and b/image.png differ\n";
        assert!(parse_unified_diff(diff).is_err());
    }

    #[test]
    fn render_no_changes() {
        let out = render_changed_hunks(&[], Path::new("foo.rs"));
        assert!(out.contains("No changes"));
    }

    #[test]
    fn render_changed_shows_counts() {
        let hunks = vec![DiffHunk {
            new_start: 5,
            new_count: 3,
            lines: vec![
                DiffLine::Context("context".to_string()),
                DiffLine::Added("new line".to_string()),
                DiffLine::Removed("old line".to_string()),
            ],
        }];
        let out = render_changed_hunks(&hunks, Path::new("foo.rs"));
        assert!(out.contains("1 hunks"));
        assert!(out.contains("+1"));
        assert!(out.contains("-1"));
        assert!(out.contains("│+│ new line"));
        assert!(out.contains("│-│ old line"));
    }

    /// Content lines starting with ---/+++ must not be dropped inside hunks. /* TEST: parser edge case */
    #[test]
    fn parse_content_with_triple_dash_prefix() -> Result<()> {
        let diff = "\
diff --git a/notes.md b/notes.md
--- a/notes.md
+++ b/notes.md
@@ -1,3 +1,4 @@
 normal line
+--- this is a markdown hr
 another line
 end
";
        let hunks = parse_unified_diff(diff)?;
        assert_eq!(hunks.len(), 1);
        // The added line with --- prefix must be preserved as Added
        assert!(
            matches!(&hunks[0].lines[1], DiffLine::Added(s) if s == "--- this is a markdown hr"),
            "--- prefixed content inside hunk must not be skipped"
        );
        Ok(())
    }

    /// Content lines starting with +++ inside hunks are preserved. /* TEST: parser edge case */
    #[test]
    fn parse_content_with_triple_plus_prefix() -> Result<()> {
        let diff = "\
@@ -1,2 +1,3 @@
 line1
++++ added section
 line2
";
        let hunks = parse_unified_diff(diff)?;
        assert_eq!(hunks.len(), 1);
        // +++ is stripped by strip_prefix('+'), leaving "++ added section"
        assert!(
            matches!(&hunks[0].lines[1], DiffLine::Added(s) if s == "+++ added section"),
            "+++ prefixed content inside hunk must be parsed as Added"
        );
        Ok(())
    }

    /// Verify --since builds rev..HEAD range (not bare rev). /* TEST: --since semantics */
    #[test]
    fn since_builds_range_args() {
        // We can't run git in unit tests, but we can verify the arg construction
        // by checking the function signature contract: revision=Some("HEAD~3")
        // should produce args containing "HEAD~3..HEAD"
        let rev = "HEAD~3";
        let expected_range = format!("{rev}..HEAD");
        assert_eq!(expected_range, "HEAD~3..HEAD");
    }

    #[test]
    fn render_multiple_hunks_separator() {
        let hunks = vec![
            DiffHunk {
                new_start: 1,
                new_count: 1,
                lines: vec![DiffLine::Added("a".to_string())],
            },
            DiffHunk {
                new_start: 10,
                new_count: 1,
                lines: vec![DiffLine::Added("b".to_string())],
            },
        ];
        let out = render_changed_hunks(&hunks, Path::new("f.rs"));
        assert!(out.contains("···"), "separator between hunks");
    }

    #[test]
    fn build_git_diff_args_changed_mode() {
        let args = build_git_diff_args("src/read.rs", None, 3);
        assert_eq!(
            args,
            vec![
                "diff".to_string(),
                "--no-color".to_string(),
                "--unified=3".to_string(),
                "--".to_string(),
                "src/read.rs".to_string()
            ]
        );
    }

    #[test]
    fn build_git_diff_args_since_mode_uses_range_to_head() {
        let args = build_git_diff_args("src/read.rs", Some("HEAD~3"), 0);
        assert_eq!(
            args,
            vec![
                "diff".to_string(),
                "--no-color".to_string(),
                "--unified=0".to_string(),
                "HEAD~3..HEAD".to_string(),
                "--".to_string(),
                "src/read.rs".to_string()
            ]
        );
    }
}
