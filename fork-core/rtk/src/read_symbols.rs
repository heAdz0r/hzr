//! Symbol model and extraction traits for `rtk read --outline/--symbols`.
//! Created in PR-3. Provides shared API for regex and tree-sitter backends.

use crate::filter::Language;
use serde::Serialize;

// ── Symbol data model ───────────────────────────────────────

/// Kind of symbol extracted from source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Interface,
    Class,
    Type,
    Constant,
    Import,
    Module,
}

/// Visibility of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    Private,
}

/// Source location span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Span {
    /// 1-based start line
    pub start_line: usize,
    /// 1-based end line (inclusive, same as start if single-line)
    pub end_line: usize,
}

/// A single extracted symbol.
#[derive(Debug, Clone, Serialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    pub span: Span,
    /// Optional parent name (e.g., impl block for methods)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Optional signature (function params, type bounds)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Versioned symbol index for JSON output stability.
#[derive(Debug, Serialize)]
pub struct SymbolIndex {
    /// Schema version for forward compatibility
    pub version: u32,
    pub language: String,
    pub total_lines: usize,
    pub symbols: Vec<Symbol>,
}

impl SymbolIndex {
    pub fn new(language: &str, total_lines: usize, symbols: Vec<Symbol>) -> Self {
        SymbolIndex {
            version: 1,
            language: language.to_string(),
            total_lines,
            symbols,
        }
    }
}

// ── Extraction trait ────────────────────────────────────────

/// Trait for symbol extraction backends.
pub trait SymbolExtractor {
    fn extract(&self, content: &str, lang: &Language) -> Vec<Symbol>;
    fn name(&self) -> &'static str;
}

// ── Outline renderer ────────────────────────────────────────

/// Render symbols as human-readable outline.
pub fn render_outline(symbols: &[Symbol], total_lines: usize) -> String {
    if symbols.is_empty() {
        return "(no symbols found)\n".to_string();
    }

    let mut out = String::new();
    let width = total_lines.to_string().len();

    for sym in symbols {
        let kind_label = match sym.kind {
            SymbolKind::Function => "fn",
            SymbolKind::Method => "fn",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Interface => "iface",
            SymbolKind::Class => "class",
            SymbolKind::Type => "type",
            SymbolKind::Constant => "const",
            SymbolKind::Import => "import",
            SymbolKind::Module => "mod",
        };

        let vis = match sym.visibility {
            Visibility::Public => "+",
            Visibility::Private => "-",
        };

        let span = if sym.span.start_line == sym.span.end_line {
            format!("L{:>w$}", sym.span.start_line, w = width)
        } else {
            format!(
                "L{:>w$}-{:>w$}",
                sym.span.start_line,
                sym.span.end_line,
                w = width
            )
        };

        let parent_prefix = match &sym.parent {
            Some(p) => format!("{p}::"),
            None => String::new(),
        };

        let sig_suffix = match &sym.signature {
            Some(s) => format!(" {s}"),
            None => String::new(),
        };

        out.push_str(&format!(
            "  {span}  {vis}{kind_label:<6} {parent_prefix}{}{sig_suffix}\n",
            sym.name
        ));
    }

    out
}

/// Render symbols as versioned JSON.
pub fn render_symbols_json(symbols: Vec<Symbol>, lang: &Language, total_lines: usize) -> String {
    let lang_name = lang_to_string(lang);
    let index = SymbolIndex::new(lang_name, total_lines, symbols);
    serde_json::to_string_pretty(&index).unwrap_or_else(|_| "{}".to_string())
}

fn lang_to_string(lang: &Language) -> &'static str {
    match lang {
        Language::Rust => "rust",
        Language::Python => "python",
        Language::JavaScript => "javascript",
        Language::TypeScript => "typescript",
        Language::Go => "go",
        Language::C => "c",
        Language::Cpp => "cpp",
        Language::Java => "java",
        Language::Ruby => "ruby",
        Language::Shell => "shell",
        Language::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_symbols() -> Vec<Symbol> {
        vec![
            Symbol {
                name: "Config".to_string(),
                kind: SymbolKind::Struct,
                visibility: Visibility::Public,
                span: Span {
                    start_line: 5,
                    end_line: 10,
                },
                parent: None,
                signature: None,
            },
            Symbol {
                name: "run".to_string(),
                kind: SymbolKind::Function,
                visibility: Visibility::Public,
                span: Span {
                    start_line: 12,
                    end_line: 30,
                },
                parent: None,
                signature: Some("(path: &Path) -> Result<()>".to_string()),
            },
            Symbol {
                name: "parse".to_string(),
                kind: SymbolKind::Method,
                visibility: Visibility::Private,
                span: Span {
                    start_line: 32,
                    end_line: 45,
                },
                parent: Some("Config".to_string()),
                signature: Some("(&self) -> String".to_string()),
            },
        ]
    }

    #[test]
    fn outline_renders_all_symbols() {
        let outline = render_outline(&sample_symbols(), 45);
        assert!(outline.contains("+struct Config"));
        assert!(outline.contains("+fn     run"));
        assert!(outline.contains("-fn     Config::parse"));
    }

    #[test]
    fn outline_empty_symbols() {
        assert_eq!(render_outline(&[], 0), "(no symbols found)\n");
    }

    #[test]
    fn json_output_has_version() {
        let json = render_symbols_json(sample_symbols(), &Language::Rust, 45);
        assert!(json.contains("\"version\": 1"));
        assert!(json.contains("\"language\": \"rust\""));
        assert!(json.contains("\"total_lines\": 45"));
    }

    #[test]
    fn symbol_index_version() {
        let idx = SymbolIndex::new("rust", 100, vec![]);
        assert_eq!(idx.version, 1);
        assert_eq!(idx.symbols.len(), 0);
    }
}
