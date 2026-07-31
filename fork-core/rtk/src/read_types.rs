//! Shared types for the `read` module family.
//! Extracted from read.rs to provide a clean contract between submodules.

use crate::filter::FilterLevel;
use std::path::PathBuf;

// ── ReadMode ────────────────────────────────────────────────

/// Target read mode, selected by CLI flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ReadMode {
    /// Default: full file read with filter+digest+cache pipeline
    #[default]
    Full,
    /// Structural outline with line spans (PR-3)
    Outline,
    /// Machine-readable JSON symbol index (PR-3)
    Symbols,
    /// Only changed hunks from git working tree (PR-5)
    Changed,
    /// Changed hunks relative to a revision (PR-5)
    Since(String),
}

// ── ReadRequest ─────────────────────────────────────────────

/// Fully resolved read parameters from CLI args.
#[derive(Debug, Clone)]
pub struct ReadRequest {
    pub file: PathBuf,
    pub level: FilterLevel,
    pub from: Option<usize>,
    pub to: Option<usize>,
    pub max_lines: Option<usize>,
    pub line_numbers: bool,
    pub mode: ReadMode,
    pub verbose: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_mode_default_is_full() {
        assert_eq!(ReadMode::default(), ReadMode::Full);
    }

    #[test]
    fn read_mode_since_holds_revision() {
        let mode = ReadMode::Since("HEAD~3".to_string());
        if let ReadMode::Since(rev) = mode {
            assert_eq!(rev, "HEAD~3");
        } else {
            panic!("expected Since variant");
        }
    }

    #[test]
    fn read_request_fields() {
        let req = ReadRequest {
            file: PathBuf::from("test.rs"),
            level: FilterLevel::Minimal,
            from: Some(10),
            to: Some(20),
            max_lines: None,
            line_numbers: true,
            mode: ReadMode::Full,
            verbose: 0,
        };
        assert_eq!(req.from, Some(10));
        assert_eq!(req.to, Some(20));
        assert!(req.line_numbers);
    }
}
