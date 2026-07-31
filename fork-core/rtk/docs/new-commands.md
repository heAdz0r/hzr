# Adding New Commands to RTK

Guide for adding new filtered commands to the rtk fork while maintaining architecture and code quality.

## Architecture Overview

```
User runs: bunx vue-tsc --noEmit
                │
                ▼
┌─ Hook (.claude/hooks/rtk-rewrite.sh) ─┐
│  bunx vue-tsc --noEmit                │
│       → rtk vue-tsc --noEmit          │
└───────────────┬───────────────────────┘
                │
                ▼
┌─ main.rs (Clap routing) ──────────────┐
│  Commands::VueTsc { args }            │
│       → tsc_cmd::run_vue_tsc(&args)   │
└───────────────┬───────────────────────┘
                │
                ▼
┌─ tsc_cmd.rs (module) ─────────────────┐
│  1. TimedExecution::start()           │
│  2. Execute: vue-tsc --noEmit         │
│  3. filter_tsc_output(&raw)           │
│  4. println!("{}", filtered)          │
│  5. timer.track(orig, rtk, raw, out)  │
│  6. process::exit(exit_code)          │
└───────────────────────────────────────┘
```

Every command follows this 4-layer pattern:
1. **Hook** rewrites raw command → `rtk <cmd>`
2. **main.rs** routes `Commands` enum → module
3. **Module** executes + filters + tracks
4. **Tracking** records token savings to SQLite

## Step-by-Step: Adding a Standalone Command

Example: adding `rtk biome` (Biome linter/formatter).

### 1. Write tests first (TDD RED)

Create `src/biome_cmd.rs` with tests at the bottom:

```rust
use crate::tracking;
use crate::utils::strip_ansi;
use anyhow::{Context, Result};
use std::process::Command;

// pub fn run() and filter will go here (step 3)

/// Filter Biome output - group diagnostics by file
pub fn filter_biome_output(output: &str) -> String {
    // implement after tests
    output.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Paste REAL output from the tool as a const
    const BIOME_OUTPUT: &str = r#"
path/to/file.ts:10:5 lint/suspicious/noExplicitAny
  ! Unexpected any. Specify a different type.
path/to/file.ts:20:3 lint/style/useConst
  ! This let can be a const.
"#;

    #[test]
    fn test_filter_biome_output() {
        let result = filter_biome_output(BIOME_OUTPUT);
        assert!(result.contains("file.ts"));
        // What MUST be in filtered output
        assert!(result.contains("noExplicitAny"));
        // What MUST NOT be in filtered output
        assert!(!result.contains("Unexpected any. Specify"));
    }

    #[test]
    fn test_filter_biome_clean() {
        let result = filter_biome_output("Checked 42 files in 0.5s. No errors found.");
        assert!(result.contains("No errors"));
    }
}
```

Run: `cargo test biome_cmd` — must **fail** (RED).

### 2. Register module in main.rs

```rust
// src/main.rs top — add mod declaration (alphabetical order)
mod biome_cmd;
```

Add to `Commands` enum:

```rust
/// Biome linter/formatter with grouped diagnostics
Biome {
    /// Biome arguments
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
},
```

Add match arm (alphabetical within the match block):

```rust
Commands::Biome { args } => {
    biome_cmd::run(&args, cli.verbose)?;
}
```

### 3. Implement run() + filter (TDD GREEN)

Minimal implementation to pass tests:

```rust
pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Use package_manager_exec for JS tools, Command::new for system tools
    let mut cmd = crate::utils::package_manager_exec("biome");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: biome {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run biome (try: npm install -g @biomejs/biome)")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_biome_output(&raw);
    println!("{}", filtered);

    timer.track(
        &format!("biome {}", args.join(" ")),
        &format!("rtk biome {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}
```

Run: `cargo test biome_cmd` — must **pass** (GREEN).

### 4. Add hook rewrite

Edit **both** hook files (keep them in sync):
- `.claude/hooks/rtk-rewrite.sh` — active hook
- `hooks/rtk-rewrite.sh` — source of truth

```bash
# Find the right section (JS/TS tooling, Python, Go, etc.) and add:
elif echo "$MATCH_CMD" | grep -qE '^(npx[[:space:]]+|bunx[[:space:]]+)?biome([[:space:]]|$)'; then
  REWRITTEN="${REWRITE_PREFIX}$(echo "$CMD_BODY" | sed -E 's/^(npx |bunx )?biome/rtk biome/')"
```

Test the hook:
```bash
echo '{"tool_input":{"command":"npx biome check src/"}}' \
  | bash .claude/hooks/rtk-rewrite.sh 2>/dev/null \
  | jq -r '.hookSpecificOutput.updatedInput.command'
# Expected: rtk biome check src/
```

### 5. Add npx routing (if applicable)

If the tool is typically invoked via `npx <tool>`, add routing in the `Commands::Npx` match block in `main.rs`:

```rust
"biome" => {
    biome_cmd::run(&args[1..], cli.verbose)?;
}
```

### 6. Pre-commit gate

```bash
cargo fmt --all --check
cargo clippy --all-targets
cargo test
```

All three must pass. Zero new warnings from your code.

### 7. Install and verify

```bash
cargo install --path .
cp ~/.cargo/bin/rtk /usr/local/bin/rtk   # sync if needed
rtk biome --help
```

### 8. Update docs

- `CLAUDE.md` — module table row + fork-specific features section
- `docs/new-commands.md` — if patterns changed

## Step-by-Step: Adding a Sub-Command

For tools with sub-commands (like `rtk go test`, `rtk go build`), use a sub-enum.

### Enum in main.rs

```rust
#[derive(Subcommand)]
enum BiomeCommands {
    /// Check with grouped diagnostics
    Check {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Format with compact output
    Format {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: runs any unsupported biome subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}
```

Top-level command references the sub-enum:

```rust
/// Biome commands with compact output
Biome {
    #[command(subcommand)]
    command: BiomeCommands,
},
```

Match dispatches to module functions:

```rust
Commands::Biome { command } => match command {
    BiomeCommands::Check { args } => {
        biome_cmd::run_check(&args, cli.verbose)?;
    }
    BiomeCommands::Format { args } => {
        biome_cmd::run_format(&args, cli.verbose)?;
    }
    BiomeCommands::Other(args) => {
        biome_cmd::run_passthrough(&args, cli.verbose)?;
    }
},
```

Reference: `GoCommands` in main.rs, `go_cmd.rs`.

## Patterns Reference

### Which execution helper to use

| Tool type | How to build `Command` | Example |
|---|---|---|
| JS/TS (eslint, prettier, biome) | `utils::package_manager_exec("tool")` | prettier_cmd.rs |
| System binary (ruff, pytest, go) | `Command::new("tool")` | ruff_cmd.rs |
| Binary with npx fallback | `which` check + `Command::new("npx")` | tsc_cmd.rs |
| Multiple runners (tsc / vue-tsc) | Parameterized `run_with_tool(tool, args)` | tsc_cmd.rs |

### Filter strategy by output type

| Output format | Parsing strategy | Example module |
|---|---|---|
| JSON (structured) | `serde_json::from_str` → group by field | lint_cmd (eslint), ruff_cmd, golangci_cmd |
| NDJSON (streaming) | Line-by-line `serde_json::from_str` | go_cmd (go test) |
| Text with patterns | `regex::Regex` + state machine | tsc_cmd, pytest_cmd |
| Text (simple) | `line.contains()` / `starts_with()` | cra_cmd, prettier_cmd |

### Grouping patterns (most token savings come from here)

```
# Group by file → count (like tsc_cmd, lint_cmd)
src/auth.ts (3 errors)
  L12: TS2322 Type 'string' not assignable...
  L15: TS2345 Argument mismatch...

# Group by rule → top N (like lint_cmd, golangci_cmd)
Top rules: no-unused-vars (12x), prefer-const (5x)

# Summary line only (like cra_cmd, prettier_cmd)
✓ CRA Build: 11 files, 182.1 kB main.js (gzip)
```

### Standard filter function signature

```rust
/// Filter <tool> output - <strategy description>
pub fn filter_<tool>_output(output: &str) -> String {
    // 1. Strip ANSI if needed: let clean = strip_ansi(output);
    // 2. Parse lines
    // 3. Group/aggregate
    // 4. Format compact output
    // 5. Return string (caller prints)
}
```

Keep filter functions `pub` — enables direct unit testing and reuse.

### Tracking boilerplate

Every command must track. Copy-paste this skeleton:

```rust
let timer = tracking::TimedExecution::start();
// ... execute command, get raw + filtered ...
timer.track(
    &format!("<original-cmd> {}", args.join(" ")),    // what user typed
    &format!("rtk <cmd> {}", args.join(" ")),          // what rtk ran
    &raw,                                               // raw output
    &filtered,                                          // filtered output
);
```

### Exit code preservation

```rust
// For commands where exit code matters (CI/CD, linters, test runners)
if !output.status.success() {
    std::process::exit(output.status.code().unwrap_or(1));
}
```

Some commands use `std::process::exit()` even on success (e.g., tsc_cmd) to always propagate. Choose based on the tool's semantics.

### Passthrough for unknown subcommands

For sub-enum commands, always add an `Other` variant:

```rust
pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let mut cmd = Command::new("biome");
    for arg in args {
        cmd.arg(arg);
    }
    let status = cmd.status().context("Failed to run biome")?;
    let args_str: Vec<String> = args.iter().map(|s| s.to_string_lossy().into_owned()).collect();
    timer.track_passthrough(
        &format!("biome {}", args_str.join(" ")),
        &format!("rtk biome {} (passthrough)", args_str.join(" ")),
    );
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
```

## Hook Rules

### Pattern structure

```bash
elif echo "$MATCH_CMD" | grep -qE '<REGEX>'; then
  REWRITTEN="${REWRITE_PREFIX}$(echo "$CMD_BODY" | sed -E '<SED_REPLACE>')"
```

### Common patterns

```bash
# Simple: tool → rtk tool
'^biome([[:space:]]|$)'
sed 's/^biome/rtk biome/'

# With npx/bunx prefix: (npx|bunx)? tool → rtk tool
'^(npx[[:space:]]+|bunx[[:space:]]+)?biome([[:space:]]|$)'
sed -E 's/^(npx |bunx )?biome/rtk biome/'

# Subcommand match: tool subcmd → rtk tool subcmd
'^biome[[:space:]]+(check|format)([[:space:]]|$)'
sed 's/^biome /rtk biome /'
```

### Critical: keep BOTH hook files in sync

```
.claude/hooks/rtk-rewrite.sh   ← active (Claude Code reads this)
hooks/rtk-rewrite.sh           ← source of truth (committed to repo)
```

### Test hooks manually

```bash
echo '{"tool_input":{"command":"npx biome check ."}}' \
  | bash .claude/hooks/rtk-rewrite.sh 2>/dev/null \
  | jq -r '.hookSpecificOutput.updatedInput.command'
```

## Test Patterns

### Test data: always use REAL output

Paste actual command output as `const` in tests. Synthetic data misses edge cases.

### What to assert

```rust
#[test]
fn test_filter_biome_output() {
    let result = filter_biome_output(REAL_OUTPUT);

    // 1. Structure: correct header/summary
    assert!(result.contains("Biome:"));

    // 2. Content preserved: key info visible
    assert!(result.contains("noExplicitAny"));
    assert!(result.contains("file.ts"));

    // 3. Noise removed: boilerplate stripped
    assert!(!result.contains("Checked 42 files"));
    assert!(!result.contains("biome.dev"));

    // 4. Compactness: output is bounded
    let lines: Vec<&str> = result.lines().collect();
    assert!(lines.len() < 20, "Too verbose: {} lines", lines.len());
}
```

### Test naming

```
test_filter_<tool>_<scenario>
test_filter_biome_output          # happy path
test_filter_biome_no_issues       # clean run
test_filter_biome_error           # compilation/runtime errors
test_filter_biome_empty           # empty/missing output
```

## Checklist

```
[ ] Tests written FIRST (RED)
[ ] Module created: src/<tool>_cmd.rs
[ ] Module registered: mod <tool>_cmd; in main.rs
[ ] Commands enum variant added
[ ] Match arm added in main()
[ ] Npx routing added (if JS/TS tool)
[ ] Filter function is pub
[ ] TimedExecution tracking in run()
[ ] Exit code preserved
[ ] Hook added to .claude/hooks/rtk-rewrite.sh
[ ] Hook added to hooks/rtk-rewrite.sh (sync!)
[ ] Hook tested manually with echo | bash | jq
[ ] cargo fmt --all --check
[ ] cargo clippy --all-targets (zero new warnings)
[ ] cargo test (all pass)
[ ] cargo install --path . && cp ~/.cargo/bin/rtk /usr/local/bin/rtk
[ ] CLAUDE.md module table updated
[ ] CLAUDE.md fork-specific features section updated
```

## File Reference

| File | Role |
|---|---|
| `src/main.rs` | CLI parsing, `Commands` enum, match routing |
| `src/<tool>_cmd.rs` | Module: `run()` + `filter_<tool>_output()` + tests |
| `src/utils.rs` | `truncate`, `strip_ansi`, `execute_command`, `package_manager_exec` |
| `src/tracking.rs` | `TimedExecution::start()` / `.track()` / `.track_passthrough()` |
| `.claude/hooks/rtk-rewrite.sh` | Active hook (Claude Code reads) |
| `hooks/rtk-rewrite.sh` | Hook source of truth (repo) |
| `CLAUDE.md` | Module table, architecture docs |
