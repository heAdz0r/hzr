//! Корневой help UX: группы команд, примеры и лёгкий оранжевый стиль HZR.
//!
//! Clap пока не умеет несколько `help_heading` для subcommands (upstream #1553 /
//! #5819), поэтому группы рендерятся через `before_help` + кастомный template.

use clap::builder::styling::{Ansi256Color, Effects, Styles};

/// Оранжевый HZR (`38;5;208`), тот же акцент, что в `stats` UX.
pub const HZR_CLI_STYLES: Styles = Styles::styled()
    .header(Ansi256Color(208).on_default().effects(Effects::BOLD))
    .usage(Ansi256Color(208).on_default().effects(Effects::BOLD))
    .literal(Ansi256Color(208).on_default().effects(Effects::BOLD))
    .placeholder(Ansi256Color(245).on_default())
    .valid(Ansi256Color(208).on_default())
    .invalid(Ansi256Color(196).on_default().effects(Effects::BOLD));

/// Template без плоского списка Commands: группы идут из `before_help`.
pub const HZR_ROOT_HELP_TEMPLATE: &str = "\
{about-with-newline}\
{usage-heading} {usage}\n\
\n\
{before-help}\
Options:\n{options}\
{after-help}\
";

/// Сгруппированный каталог top-level команд (короткие about).
pub const HZR_COMMAND_GROUPS: &str = "\
Setup:
  init       Register workspace data layout and service
  enable     Enable HZR for one workspace
  disable    Disable HZR for one workspace; keep data
  activation Inspect project-only activation and enabled workspaces
  install    Adopt HZR binaries, hooks, and instructions
  uninstall  Remove HZR adoption hooks
  hooks      Inspect or run the HZR hook dispatcher
  doctor     Verify pins, ownership, and daemon health

Runtime:
  daemon     Operate the authenticated loopback daemon
  engines    Inspect the engine manifest from hzrd
  index      Inspect or init the canonical grepai index

Search & Memory:
  search     Search via the exact/semantic router
  rgai       rgai-compatible search over the same index
  context    Plan bounded code and ICM context
  memory     Recall, store, or inspect ICM memory

Agent tools:
  exec       Rewrite or run a command through policy
  codec      Compile protected response-density transforms
  agent      Run the managed caveman-code agent
  mcp        Serve HZR tools over stdio MCP
  tdd        Print the HZR Red-Green-Refactor contract

Distribution:
  release    Build, install, and verify an HZR release
  update     Install a newer GitHub release when available
  build      Build your project (token-optimized wrapper)
  stats      Show cumulative efficiency gains

Legacy:
  migrate    Inspect or centralize legacy state
  rtk        Run the managed fork-core CLI unchanged
";

/// Короткий Examples footer для типичного пути.
pub const HZR_EXAMPLES_FOOTER: &str = "\
Examples:
  hzr init
  hzr install
  hzr doctor
  hzr search \"auth flow\"
  hzr stats
  hzr memory recall \"decisions\"
";
