use crate::tracking;
use anyhow::{Context, Result};
use std::io::IsTerminal;
use std::process::Command;

/// Noise directories commonly excluded from LLM context
const NOISE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "__pycache__",
    ".next",
    "dist",
    "build",
    ".cache",
    ".turbo",
    ".vercel",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    ".venv",
    "venv",
    "coverage",
    ".nyc_output",
    ".DS_Store",
    "Thumbs.db",
    ".idea",
    ".vscode",
    ".vs",
    "*.egg-info",
    ".eggs",
];

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Separate flags from paths
    let show_all = args
        .iter()
        .any(|a| (a.starts_with('-') && !a.starts_with("--") && a.contains('a')) || a == "--all");
    // 0.11.0 (US-014, upstream aa40853/a7e7329): standard dotfile semantics — `-a`
    // and `-A` show dot entries, plain `ls` does not. `-A` still filters noise dirs.
    let show_dot = show_all
        || args
            .iter()
            .any(|a| (a.starts_with('-') && !a.starts_with("--") && a.contains('A')) || a == "--almost-all");

    let flags: Vec<&str> = args
        .iter()
        .filter(|a| a.starts_with('-'))
        .map(|s| s.as_str())
        .collect();
    let paths: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .collect();

    // 0.11.0 (US-014): several operands, `-R` or `-d` print sections or the
    // directory itself; the one-list compaction merged them. Native ls answers.
    let short_flags: String = flags
        .iter()
        .filter(|f| !f.starts_with("--"))
        .flat_map(|f| f.chars().skip(1))
        .collect();
    if paths.len() > 1
        || short_flags.contains(['R', 'd'])
        || short_flags.contains(['C', 'x', 'm']) // 0.11.2: column/comma layouts are native's
        || flags.contains(&"--recursive")
        || flags.contains(&"--directory")
    {
        return run_native(args);
    }
    // 0.11.2: `-l` asks for the long columns; they are kept, only the padding is squeezed.
    let long = short_flags.contains('l')
        || flags.contains(&"--format=long")
        || flags.contains(&"--format=verbose"); // 0.11.2

    // Build ls -la + any extra flags the user passed (e.g. -R)
    // Strip -l, -a, -h (we handle all of these ourselves)
    let mut cmd = Command::new("ls");
    cmd.arg("-la");
    // 0.11.0 (upstream PR #3651): plain `ls link` lists the directory a command-line
    // symlink points to; the injected `-l` would show the link itself. Follow
    // command-line symlinks unless the caller asked for the long format.
    if !short_flags.contains('l') && !flags.contains(&"--format=long") {
        cmd.arg("-H");
    }
    for flag in &flags {
        if flag.starts_with("--") {
            // Long flags: skip --all (already handled)
            if *flag != "--all" {
                cmd.arg(flag);
            }
        } else {
            let stripped = flag.trim_start_matches('-');
            let extra: String = stripped
                .chars()
                // 0.11.2: -lh keeps human sizes; -1 (one per line) is dropped — it overrode
                // the injected -l and every `ls -1` came back "(empty)".
                .filter(|c| *c != 'l' && *c != 'a' && *c != '1' && (*c != 'h' || long))
                .collect();
            if !extra.is_empty() {
                cmd.arg(format!("-{}", extra));
            }
        }
    }

    // Add paths (default to "." if none)
    if paths.is_empty() {
        cmd.arg(".");
    } else {
        for p in &paths {
            cmd.arg(p);
        }
    }

    let output = cmd.output().context("Failed to run ls")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprint!("{}", stderr);
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let target = paths.first().copied().unwrap_or(".");
    let filtered = if long {
        compact_long(&raw, show_all, show_dot, target) // 0.11.2
    } else {
        compact_ls(&raw, show_all, show_dot, std::io::stdout().is_terminal(), target) // 0.11.2
    };
    // 0.11.2: the baseline is what the caller's own `ls` prints, not rtk's internal `ls -la`.
    let baseline = native_view(&raw, long, show_all, show_dot);

    if verbose > 0 {
        eprintln!(
            "Chars: {} → {} ({}% reduction)",
            raw.len(),
            filtered.len(),
            if !raw.is_empty() {
                100 - (filtered.len() * 100 / raw.len())
            } else {
                0
            }
        );
    }

    let target_display = if paths.is_empty() {
        ".".to_string()
    } else {
        paths.join(" ")
    };
    // 0.11.2: never more than the caller's own `ls` prints. A small directory's compact
    // form (sizes, noise footer) is larger than its plain names, so the names win there.
    let shown = crate::guard::never_worse_content(&baseline, &filtered);
    print!("{}", shown); // 0.11.2
    timer.track(
        &format!("ls -la {}", target_display),
        "rtk ls",
        &baseline, // 0.11.2
        shown,     // 0.11.2
    );

    Ok(())
}

/// Format bytes into human-readable size
fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1}M", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}

/// Native `ls` with the caller's own arguments, output untouched.
fn run_native(args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let status = Command::new("ls")
        .args(args)
        .status()
        .context("Failed to run ls")?;
    let label = format!("ls {}", args.join(" "));
    timer.track_passthrough(&label, &format!("rtk {label} (passthrough)"));
    let code = crate::stream::status_to_exit_code(status);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// 0.11.2: the eight metadata columns of an `ls -l` line and the name as printed.
fn split_long_line(line: &str) -> Option<(Vec<&str>, &str)> {
    let mut columns = Vec::with_capacity(8); // 0.11.2
    let mut rest = line; // 0.11.2
    for _ in 0..8 {
        let trimmed = rest.trim_start(); // 0.11.2
        let end = trimmed.find(char::is_whitespace)?; // 0.11.2
        columns.push(&trimmed[..end]); // 0.11.2
        rest = &trimmed[end..]; // 0.11.2
    }
    let name = rest.trim_start(); // 0.11.2
    (!name.is_empty()).then_some((columns, name)) // 0.11.2
}

/// 0.11.2: the listing the caller's own command prints, rendered from rtk's `ls -la`:
/// the long lines for `-l`, otherwise one name per line (ls's captured-output form).
fn native_view(raw: &str, long: bool, show_all: bool, show_dot: bool) -> String {
    let mut view = String::new(); // 0.11.2
    for line in raw.lines() {
        if line.starts_with("total ") {
            if long {
                view.push_str(line); // 0.11.2
                view.push('\n'); // 0.11.2
            }
            continue;
        }
        let Some((columns, name)) = split_long_line(line) else {
            continue; // 0.11.2
        };
        let listed = if show_all {
            true // 0.11.2: -a
        } else if show_dot {
            name != "." && name != ".." // 0.11.2: -A
        } else {
            !name.starts_with('.') // 0.11.2
        };
        if !listed {
            continue;
        }
        if long {
            view.push_str(line); // 0.11.2
        } else if columns[0].starts_with('l') {
            view.push_str(name.split(" -> ").next().unwrap_or(name)); // 0.11.2
        } else {
            view.push_str(name); // 0.11.2
        }
        view.push('\n'); // 0.11.2
    }
    view // 0.11.2
}

/// 0.11.2: `ls -l` keeps every column the caller asked for, in ls's own order (so `-t`
/// and `-S` still sort); only the padding is squeezed and noise directories are named.
fn compact_long(raw: &str, show_all: bool, show_dot: bool, target: &str) -> String {
    let ignore_dirs = crate::config::Config::merged_ignore_dirs(NOISE_DIRS); // 0.11.2
    let mut out = String::new(); // 0.11.2
    let mut hidden_names: Vec<&str> = Vec::new(); // 0.11.2
    let (mut listed, mut total) = (0usize, 0usize); // 0.11.2
    for line in raw.lines() {
        let Some((columns, name)) = split_long_line(line) else {
            continue; // 0.11.2: the `total` line
        };
        if name == "." || name == ".." || (!show_dot && name.starts_with('.')) {
            continue; // 0.11.2
        }
        if !show_all && ignore_dirs.iter().any(|dir| dir == name) {
            hidden_names.push(name); // 0.11.2
            continue;
        }
        total += 1; // 0.11.2
        if listed == LS_MAX_ENTRIES {
            continue;
        }
        out.push_str(&columns.join(" ")); // 0.11.2
        out.push(' '); // 0.11.2
        out.push_str(name); // 0.11.2
        out.push('\n'); // 0.11.2
        listed += 1; // 0.11.2
    }
    if total == 0 && hidden_names.is_empty() {
        return "(empty)\n".to_string(); // 0.11.2
    }
    if listed < total {
        out.push_str(&format!(
            "+{} more; full listing: HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=complete_log hzr exec run 'ls -la {}'\n",
            total - listed,
            target.replace('\'', "'\\''")
        )); // 0.11.2
    }
    if !hidden_names.is_empty() {
        out.push_str(&format!(
            "({} noise hidden: {}; -a shows)\n",
            hidden_names.len(),
            hidden_names.join(", ")
        )); // 0.11.2
    }
    out // 0.11.2
}

/// Entries listed before the rest goes behind the recovery line. (0.11.0, US-014)
const LS_MAX_ENTRIES: usize = 200;

/// Parse ls -la output into compact format:
///   name/  (dirs)
///   name  size  (files)
fn compact_ls(
    raw: &str,
    show_all: bool,
    show_dot: bool,
    include_summary: bool,
    target: &str,
) -> String {
    use std::collections::HashMap;

    let ignore_dirs = crate::config::Config::merged_ignore_dirs(NOISE_DIRS);
    let mut hidden = 0usize;
    let mut hidden_names: Vec<String> = Vec::new(); // 0.11.0 (US-014)
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new(); // (name, size)
    let mut by_ext: HashMap<String, usize> = HashMap::new();

    for line in raw.lines() {
        // Skip total, empty, . and ..
        if line.starts_with("total ") || line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }

        // Filename is everything from column 9 onward (handles spaces)
        let name = parts[8..].join(" ");

        // Skip . and ..
        if name == "." || name == ".." {
            continue;
        }

        // 0.11.0 (US-014): without -a/-A, dot entries are not listed — like ls.
        if !show_dot && name.starts_with('.') {
            continue;
        }

        // Filter noise dirs unless -a. The list is the built-in noise set merged
        // with the user's configured `[filters].ignore_dirs`.
        if !show_all && ignore_dirs.contains(&name) {
            hidden += 1;
            hidden_names.push(name);
            continue;
        }

        let is_dir = parts[0].starts_with('d');

        if is_dir {
            dirs.push(name);
        } else if parts[0].starts_with('-') || parts[0].starts_with('l') {
            let size: u64 = parts[4].parse().unwrap_or(0);
            let ext = if let Some(pos) = name.rfind('.') {
                name[pos..].to_string()
            } else {
                "no ext".to_string()
            };
            *by_ext.entry(ext).or_insert(0) += 1;
            files.push((name, human_size(size)));
        }
    }

    if dirs.is_empty() && files.is_empty() {
        // `(empty)` must mean empty. When every entry was filtered, say so and
        // name the lever, or the agent concludes the directory has no content.
        if hidden > 0 {
            return format!("({} hidden, use -a)\n", hidden);
        }
        return "(empty)\n".to_string();
    }

    let mut out = String::new();
    let total_entries = dirs.len() + files.len();
    let mut listed = 0usize;

    // Dirs first, compact
    for d in &dirs {
        if listed == LS_MAX_ENTRIES {
            break;
        }
        out.push_str(d);
        out.push_str("/\n");
        listed += 1;
    }

    // Files with size
    for (name, size) in &files {
        if listed == LS_MAX_ENTRIES {
            break;
        }
        out.push_str(name);
        out.push_str("  ");
        out.push_str(size);
        out.push('\n');
        listed += 1;
    }

    // 0.11.0 (US-014): what rtk left out is named, with the way back.
    if listed < total_entries {
        out.push_str(&format!(
            "+{} more; full listing: HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=complete_log hzr exec run 'ls -la {}'\n",
            total_entries - listed,
            target.replace('\'', "'\\''")
        ));
    }
    if hidden > 0 {
        out.push_str(&format!("({} noise hidden: {}; -a shows)\n", hidden, hidden_names.join(", ")));
    }

    if include_summary {
        out.push('\n');
        let mut summary = format!("📊 {} files, {} dirs", files.len(), dirs.len());
        if !by_ext.is_empty() {
            let mut ext_counts: Vec<_> = by_ext.iter().collect();
            ext_counts.sort_by(|a, b| b.1.cmp(a.1));
            let ext_parts: Vec<String> = ext_counts
                .iter()
                .take(5)
                .map(|(ext, count)| format!("{} {}", count, ext))
                .collect();
            summary.push_str(" (");
            summary.push_str(&ext_parts.join(", "));
            if ext_counts.len() > 5 {
                summary.push_str(&format!(", +{} more", ext_counts.len() - 5));
            }
            summary.push(')');
        }
        out.push_str(&summary);
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // 0.11.0 (US-014): dotfile semantics, noise disclosure and the entry cap.
    const LS_WITH_DOTS: &str = "total 8\n\
drwxr-xr-x  5 u g  160 Jan  1 00:00 .\n\
drwxr-xr-x  9 u g  288 Jan  1 00:00 ..\n\
-rw-r--r--  1 u g   12 Jan  1 00:00 .env\n\
drwxr-xr-x  3 u g   96 Jan  1 00:00 .github\n\
drwxr-xr-x  3 u g   96 Jan  1 00:00 node_modules\n\
drwxr-xr-x  3 u g   96 Jan  1 00:00 src\n\
-rw-r--r--  1 u g  100 Jan  1 00:00 main.rs\n";

    // 0.11.2: the recorded baseline is what the caller's own `ls` prints.
    #[cfg(unix)]
    fn real_ls(args: &[&str], dir: &std::path::Path) -> String {
        let output = Command::new("ls").args(args).arg(dir).output().expect("ls"); // 0.11.2
        String::from_utf8(output.stdout).expect("utf-8 listing") // 0.11.2
    }

    #[cfg(unix)]
    #[test]
    fn baseline_is_what_the_callers_ls_prints() {
        let dir = tempfile::tempdir().expect("tempdir"); // 0.11.2
        std::fs::write(dir.path().join("a.txt"), "x").expect("file"); // 0.11.2
        std::fs::write(dir.path().join(".env"), "x").expect("dotfile"); // 0.11.2
        std::fs::write(dir.path().join("my file.rs"), "x").expect("spaced"); // 0.11.2
        std::fs::create_dir(dir.path().join("src")).expect("dir"); // 0.11.2
        std::os::unix::fs::symlink("a.txt", dir.path().join("link")).expect("symlink"); // 0.11.2
        let internal = real_ls(&["-la"], dir.path()); // 0.11.2
        assert_eq!(native_view(&internal, false, false, false), real_ls(&[], dir.path())); // 0.11.2
        assert_eq!(native_view(&internal, false, false, true), real_ls(&["-A"], dir.path())); // 0.11.2
        assert_eq!(native_view(&internal, false, true, true), real_ls(&["-a"], dir.path())); // 0.11.2
        let entries = |listing: &str| -> Vec<String> {
            listing
                .lines()
                .filter(|l| !l.starts_with("total "))
                .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
                .collect()
        }; // 0.11.2: column padding depends on the widest row, `..` included
        assert_eq!(
            entries(&native_view(&internal, true, false, false)),
            entries(&real_ls(&["-l"], dir.path()))
        ); // 0.11.2
        assert!(internal.len() > 3 * real_ls(&[], dir.path()).len(), "fixture shows the inflation"); // 0.11.2
    }

    #[test]
    fn long_format_keeps_permissions_owner_size_and_dates() {
        let out = compact_long(LS_WITH_DOTS, false, false, "."); // 0.11.2
        assert!(out.contains("-rw-r--r-- 1 u g 100 Jan 1 00:00 main.rs"), "{out}"); // 0.11.2
        assert!(out.contains("drwxr-xr-x 3 u g 96 Jan 1 00:00 src"), "{out}"); // 0.11.2
        assert!(!out.contains(".env") && !out.contains("total"), "{out}"); // 0.11.2
        assert!(out.contains("(1 noise hidden: node_modules; -a shows)"), "{out}"); // 0.11.2
        let all = compact_long(LS_WITH_DOTS, true, true, "."); // 0.11.2
        assert!(all.contains("-rw-r--r-- 1 u g 12 Jan 1 00:00 .env"), "{all}"); // 0.11.2
        assert!(all.contains("node_modules") && !all.contains("noise hidden"), "{all}"); // 0.11.2
    }

    #[test]
    fn plain_ls_hides_dot_entries_and_names_hidden_noise() {
        let out = compact_ls(LS_WITH_DOTS, false, false, false, ".");
        assert!(!out.contains(".env") && !out.contains(".github"), "{out}");
        assert!(out.contains("src/") && out.contains("main.rs"), "{out}");
        assert!(out.contains("(1 noise hidden: node_modules; -a shows)"), "{out}");
        let almost_all = compact_ls(LS_WITH_DOTS, false, true, false, ".");
        assert!(almost_all.contains(".env") && almost_all.contains(".github/"), "{almost_all}");
        assert!(!almost_all.contains("node_modules/"), "{almost_all}");
        let all = compact_ls(LS_WITH_DOTS, true, true, false, ".");
        assert!(all.contains("node_modules/") && !all.contains("noise hidden"), "{all}");
    }

    #[test]
    fn huge_listings_are_capped_with_recovery() {
        let mut raw = String::from("total 1\n");
        for i in 0..(LS_MAX_ENTRIES + 25) {
            raw.push_str(&format!("-rw-r--r--  1 u g  1 Jan  1 00:00 f{i}.txt\n"));
        }
        let out = compact_ls(&raw, false, false, false, "big dir");
        assert_eq!(out.lines().filter(|l| l.starts_with('f')).count(), LS_MAX_ENTRIES);
        assert!(out.contains("+25 more; full listing:"), "{out}");
        assert!(out.contains("hzr exec run 'ls -la big dir'"), "{out}");
    }

    #[test]
    fn test_compact_basic() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 .\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 ..\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 Cargo.toml\n\
                     -rw-r--r--  1 user  staff  5678 Jan  1 12:00 README.md\n";
        let output = compact_ls(input, false, true, true, ".");
        assert!(output.contains("src/"));
        assert!(output.contains("Cargo.toml"));
        assert!(output.contains("README.md"));
        assert!(output.contains("1.2K")); // 1234 bytes
        assert!(output.contains("5.5K")); // 5678 bytes
        assert!(!output.contains("drwx")); // no permissions
        assert!(!output.contains("staff")); // no group
        assert!(!output.contains("total")); // no total
        assert!(!output.contains("\n.\n")); // no . entry
        assert!(!output.contains("\n..\n")); // no .. entry
    }

    #[test]
    fn test_compact_filters_noise() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 target\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let output = compact_ls(input, false, true, true, ".");
        // 0.11.0 (US-014): not listed as entries, but named in the disclosure line.
        assert!(!output.contains("node_modules/"));
        assert!(!output.contains(".git/"));
        assert!(!output.contains("target/"));
        assert!(output.contains("(3 noise hidden: node_modules, .git, target; -a shows)"), "{output}");
        assert!(output.contains("src/"));
        assert!(output.contains("main.rs"));
    }

    #[test]
    fn test_compact_show_all() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n";
        let output = compact_ls(input, true, true, true, ".");
        assert!(output.contains(".git/"));
        assert!(output.contains("src/"));
    }

    #[test]
    fn test_compact_empty() {
        let input = "total 0\n";
        let output = compact_ls(input, false, true, true, ".");
        assert_eq!(output, "(empty)\n");
    }

    #[test]
    fn test_compact_summary() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n\
                     -rw-r--r--  1 user  staff  5678 Jan  1 12:00 lib.rs\n\
                     -rw-r--r--  1 user  staff   100 Jan  1 12:00 Cargo.toml\n";
        let output = compact_ls(input, false, true, true, ".");
        assert!(output.contains("📊 3 files, 1 dirs"));
        assert!(output.contains(".rs"));
        assert!(output.contains(".toml"));
    }

    #[test]
    fn test_compact_suppresses_summary_for_captured_output() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let output = compact_ls(input, false, true, false, ".");
        assert_eq!(output, "src/\nmain.rs  100B\n");
        assert!(!output.contains("📊"));
    }

    #[test]
    fn test_human_size() {
        assert_eq!(human_size(0), "0B");
        assert_eq!(human_size(500), "500B");
        assert_eq!(human_size(1024), "1.0K");
        assert_eq!(human_size(1234), "1.2K");
        assert_eq!(human_size(1_048_576), "1.0M");
        assert_eq!(human_size(2_500_000), "2.4M");
    }

    #[test]
    fn test_compact_handles_filenames_with_spaces() {
        let input = "total 8\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 my file.txt\n";
        let output = compact_ls(input, false, true, true, ".");
        assert!(output.contains("my file.txt"));
    }

    #[test]
    fn test_compact_symlinks() {
        let input = "total 8\n\
                     lrwxr-xr-x  1 user  staff  10 Jan  1 12:00 link -> target\n";
        let output = compact_ls(input, false, true, true, ".");
        assert!(output.contains("link -> target"));
    }
}
