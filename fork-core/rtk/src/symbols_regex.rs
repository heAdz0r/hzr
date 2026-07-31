//! Regex-based symbol extractor for `rtk read --outline/--symbols`.
//! Created in PR-3. Supports Rust, Python, TypeScript/JavaScript, Go, Java.

use crate::filter::Language;
use crate::read_symbols::{Span, Symbol, SymbolExtractor, SymbolKind, Visibility};
use regex::Regex;

pub struct RegexExtractor;

impl SymbolExtractor for RegexExtractor {
    fn extract(&self, content: &str, lang: &Language) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        match lang {
            Language::Rust => extract_rust(&lines, &mut symbols),
            Language::Python => extract_python(&lines, &mut symbols),
            Language::TypeScript => extract_typescript(&lines, &mut symbols),
            Language::JavaScript => extract_javascript(&lines, &mut symbols),
            Language::Go => extract_go(&lines, &mut symbols),
            Language::Java => extract_java(&lines, &mut symbols),
            _ => {} // unsupported languages return empty
        }

        // Compute end_line spans by scanning for closing braces
        compute_spans(&lines, &mut symbols, lang);

        symbols
    }

    fn name(&self) -> &'static str {
        "regex"
    }
}

// ── Rust extraction ─────────────────────────────────────────

fn extract_rust(lines: &[&str], symbols: &mut Vec<Symbol>) {
    let re_fn =
        Regex::new(r"^\s*(pub(?:\(crate\))?\s+)?(?:async\s+)?fn\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*(?:<[^>]*>\s*)?\(([^)]*)\)(?:\s*->\s*(.+?))?(?:\s*\{|\s*where)")
            .unwrap();
    let re_struct =
        Regex::new(r"^\s*(pub(?:\(crate\))?\s+)?struct\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap();
    let re_enum =
        Regex::new(r"^\s*(pub(?:\(crate\))?\s+)?enum\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap();
    let re_trait =
        Regex::new(r"^\s*(pub(?:\(crate\))?\s+)?trait\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap();
    let re_type =
        Regex::new(r"^\s*(pub(?:\(crate\))?\s+)?type\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap();
    let re_const =
        Regex::new(r"^\s*(pub(?:\(crate\))?\s+)?(?:const|static)\s+([A-Z_][A-Z0-9_]*)\s*:")
            .unwrap();
    let re_mod = Regex::new(r"^\s*(pub(?:\(crate\))?\s+)?mod\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap();
    let re_impl = Regex::new(
        r"^\s*impl(?:<[^>]*>)?\s+(?:([a-zA-Z_][a-zA-Z0-9_:]*)\s+for\s+)?([a-zA-Z_][a-zA-Z0-9_]*)",
    )
    .unwrap();

    let mut current_impl: Option<String> = None;
    let mut impl_brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        // Track impl blocks for method parent assignment
        if let Some(caps) = re_impl.captures(line) {
            let type_name = caps.get(2).unwrap().as_str().to_string();
            current_impl = Some(type_name);
            impl_brace_depth = 0;
            // Count braces on impl line
            for ch in line.chars() {
                match ch {
                    '{' => impl_brace_depth += 1,
                    '}' => impl_brace_depth -= 1,
                    _ => {}
                }
            }
            continue;
        }

        // Track brace depth for impl blocks
        if current_impl.is_some() {
            for ch in line.chars() {
                match ch {
                    '{' => impl_brace_depth += 1,
                    '}' => impl_brace_depth -= 1,
                    _ => {}
                }
            }
            if impl_brace_depth <= 0 {
                current_impl = None;
            }
        }

        if let Some(caps) = re_fn.captures(line) {
            let vis = if caps.get(1).is_some() {
                Visibility::Public
            } else {
                Visibility::Private
            };
            let name = caps[2].to_string();
            let params = caps[3].trim().to_string();
            let ret = caps.get(4).map(|m| m.as_str().trim().to_string());

            let sig = if let Some(r) = ret {
                format!("({params}) -> {r}")
            } else {
                format!("({params})")
            };

            let kind = if current_impl.is_some() {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };

            symbols.push(Symbol {
                name,
                kind,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: current_impl.clone(),
                signature: Some(sig),
            });
            continue;
        }

        if let Some(caps) = re_struct.captures(line) {
            let vis = if caps.get(1).is_some() {
                Visibility::Public
            } else {
                Visibility::Private
            };
            symbols.push(Symbol {
                name: caps[2].to_string(),
                kind: SymbolKind::Struct,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        if let Some(caps) = re_enum.captures(line) {
            let vis = if caps.get(1).is_some() {
                Visibility::Public
            } else {
                Visibility::Private
            };
            symbols.push(Symbol {
                name: caps[2].to_string(),
                kind: SymbolKind::Enum,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        if let Some(caps) = re_trait.captures(line) {
            let vis = if caps.get(1).is_some() {
                Visibility::Public
            } else {
                Visibility::Private
            };
            symbols.push(Symbol {
                name: caps[2].to_string(),
                kind: SymbolKind::Trait,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        if let Some(caps) = re_type.captures(line) {
            let vis = if caps.get(1).is_some() {
                Visibility::Public
            } else {
                Visibility::Private
            };
            symbols.push(Symbol {
                name: caps[2].to_string(),
                kind: SymbolKind::Type,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        if let Some(caps) = re_const.captures(line) {
            let vis = if caps.get(1).is_some() {
                Visibility::Public
            } else {
                Visibility::Private
            };
            symbols.push(Symbol {
                name: caps[2].to_string(),
                kind: SymbolKind::Constant,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        if let Some(caps) = re_mod.captures(line) {
            let vis = if caps.get(1).is_some() {
                Visibility::Public
            } else {
                Visibility::Private
            };
            symbols.push(Symbol {
                name: caps[2].to_string(),
                kind: SymbolKind::Module,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
        }
    }
}

// ── Python extraction ───────────────────────────────────────

fn extract_python(lines: &[&str], symbols: &mut Vec<Symbol>) {
    let re_class = Regex::new(r"^class\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap();
    let re_fn = Regex::new(r"^(\s*)def\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(([^)]*)\)").unwrap();

    let mut current_class: Option<String> = None;

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        if let Some(caps) = re_class.captures(line) {
            let name = caps[1].to_string();
            current_class = Some(name.clone());
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Class,
                visibility: Visibility::Public,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        if let Some(caps) = re_fn.captures(line) {
            let indent = caps[1].len();
            let name = caps[2].to_string();
            let params = caps[3].trim().to_string();

            let (kind, parent, vis) = if indent > 0 && current_class.is_some() {
                let v = if name.starts_with('_') && !name.starts_with("__") {
                    Visibility::Private
                } else {
                    Visibility::Public
                };
                (SymbolKind::Method, current_class.clone(), v)
            } else {
                // Reset class context on unindented def
                if indent == 0 {
                    current_class = None;
                }
                let v = if name.starts_with('_') {
                    Visibility::Private
                } else {
                    Visibility::Public
                };
                (SymbolKind::Function, None, v)
            };

            symbols.push(Symbol {
                name,
                kind,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent,
                signature: Some(format!("({params})")),
            });
        }
    }
}

// ── TypeScript extraction ───────────────────────────────────

fn extract_typescript(lines: &[&str], symbols: &mut Vec<Symbol>) {
    extract_js_ts_common(lines, symbols, true);
}

// ── JavaScript extraction ───────────────────────────────────

fn extract_javascript(lines: &[&str], symbols: &mut Vec<Symbol>) {
    extract_js_ts_common(lines, symbols, false);
}

fn extract_js_ts_common(lines: &[&str], symbols: &mut Vec<Symbol>, is_ts: bool) {
    let re_fn = Regex::new(
        r"^(?:export\s+)?(?:async\s+)?function\s+([a-zA-Z_$][a-zA-Z0-9_$]*)\s*\(([^)]*)\)",
    )
    .unwrap();
    let re_arrow = Regex::new(
        r"^(?:export\s+)?(?:const|let|var)\s+([a-zA-Z_$][a-zA-Z0-9_$]*)\s*=\s*(?:async\s+)?(?:\([^)]*\)|[a-zA-Z_$][a-zA-Z0-9_$]*)\s*(?::\s*[^=]+)?\s*=>",
    )
    .unwrap();
    let re_class =
        Regex::new(r"^(?:export\s+)?(?:abstract\s+)?class\s+([a-zA-Z_$][a-zA-Z0-9_$]*)").unwrap();

    // TS-only patterns
    let re_interface = if is_ts {
        Some(Regex::new(r"^(?:export\s+)?interface\s+([a-zA-Z_$][a-zA-Z0-9_$]*)").unwrap())
    } else {
        None
    };
    let re_type = if is_ts {
        Some(Regex::new(r"^(?:export\s+)?type\s+([a-zA-Z_$][a-zA-Z0-9_$]*)\s*[=<]").unwrap())
    } else {
        None
    };
    let re_enum = if is_ts {
        Some(Regex::new(r"^(?:export\s+)?(?:const\s+)?enum\s+([a-zA-Z_$][a-zA-Z0-9_$]*)").unwrap())
    } else {
        None
    };

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();
        let vis = if trimmed.starts_with("export") {
            Visibility::Public
        } else {
            Visibility::Private
        };

        if let Some(caps) = re_fn.captures(trimmed) {
            symbols.push(Symbol {
                name: caps[1].to_string(),
                kind: SymbolKind::Function,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: Some(format!("({})", &caps[2])),
            });
            continue;
        }

        if let Some(caps) = re_arrow.captures(trimmed) {
            symbols.push(Symbol {
                name: caps[1].to_string(),
                kind: SymbolKind::Function,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        if let Some(caps) = re_class.captures(trimmed) {
            symbols.push(Symbol {
                name: caps[1].to_string(),
                kind: SymbolKind::Class,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        if let Some(ref re) = re_interface {
            if let Some(caps) = re.captures(trimmed) {
                symbols.push(Symbol {
                    name: caps[1].to_string(),
                    kind: SymbolKind::Interface,
                    visibility: vis,
                    span: Span {
                        start_line: line_num,
                        end_line: line_num,
                    },
                    parent: None,
                    signature: None,
                });
                continue;
            }
        }

        if let Some(ref re) = re_type {
            if let Some(caps) = re.captures(trimmed) {
                symbols.push(Symbol {
                    name: caps[1].to_string(),
                    kind: SymbolKind::Type,
                    visibility: vis,
                    span: Span {
                        start_line: line_num,
                        end_line: line_num,
                    },
                    parent: None,
                    signature: None,
                });
                continue;
            }
        }

        if let Some(ref re) = re_enum {
            if let Some(caps) = re.captures(trimmed) {
                symbols.push(Symbol {
                    name: caps[1].to_string(),
                    kind: SymbolKind::Enum,
                    visibility: vis,
                    span: Span {
                        start_line: line_num,
                        end_line: line_num,
                    },
                    parent: None,
                    signature: None,
                });
                continue;
            }
        }
    }
}

// ── Go extraction ───────────────────────────────────────────

fn extract_go(lines: &[&str], symbols: &mut Vec<Symbol>) {
    let re_fn = Regex::new(r"^func\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(([^)]*)\)").unwrap();
    let re_method =
        Regex::new(r"^func\s+\(([^)]+)\)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(([^)]*)\)").unwrap();
    let re_struct = Regex::new(r"^type\s+([a-zA-Z_][a-zA-Z0-9_]*)\s+struct\b").unwrap();
    let re_interface = Regex::new(r"^type\s+([a-zA-Z_][a-zA-Z0-9_]*)\s+interface\b").unwrap();
    let re_type = Regex::new(r"^type\s+([a-zA-Z_][a-zA-Z0-9_]*)\s+").unwrap();

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        // Method (with receiver) must be checked before function
        if let Some(caps) = re_method.captures(line) {
            let receiver = caps[1].trim().to_string();
            let name = caps[2].to_string();
            let params = caps[3].trim().to_string();
            let vis = if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                Visibility::Public
            } else {
                Visibility::Private
            };
            // Extract type name from receiver
            let parent = receiver
                .split_whitespace()
                .last()
                .map(|s| s.trim_start_matches('*').to_string());
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Method,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent,
                signature: Some(format!("({params})")),
            });
            continue;
        }

        if let Some(caps) = re_fn.captures(line) {
            let name = caps[1].to_string();
            let params = caps[2].trim().to_string();
            let vis = if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                Visibility::Public
            } else {
                Visibility::Private
            };
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Function,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: Some(format!("({params})")),
            });
            continue;
        }

        if let Some(caps) = re_struct.captures(line) {
            let name = caps[1].to_string();
            let vis = if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                Visibility::Public
            } else {
                Visibility::Private
            };
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Struct,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        if let Some(caps) = re_interface.captures(line) {
            let name = caps[1].to_string();
            let vis = if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                Visibility::Public
            } else {
                Visibility::Private
            };
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Interface,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        // Generic type aliases (not struct/interface)
        if re_type.is_match(line) && !re_struct.is_match(line) && !re_interface.is_match(line) {
            if let Some(caps) = re_type.captures(line) {
                let name = caps[1].to_string();
                let vis = if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                    Visibility::Public
                } else {
                    Visibility::Private
                };
                symbols.push(Symbol {
                    name,
                    kind: SymbolKind::Type,
                    visibility: vis,
                    span: Span {
                        start_line: line_num,
                        end_line: line_num,
                    },
                    parent: None,
                    signature: None,
                });
            }
        }
    }
}

// ── Java extraction ─────────────────────────────────────────

fn extract_java(lines: &[&str], symbols: &mut Vec<Symbol>) {
    let re_class = Regex::new(
        r"^\s*(?:public|protected|private)?\s*(?:abstract\s+)?(?:final\s+)?class\s+([a-zA-Z_][a-zA-Z0-9_]*)",
    )
    .unwrap();
    let re_interface =
        Regex::new(r"^\s*(?:public|protected|private)?\s*interface\s+([a-zA-Z_][a-zA-Z0-9_]*)")
            .unwrap();
    let re_method = Regex::new(
        r"^\s*(public|protected|private)?\s*(?:static\s+)?(?:final\s+)?(?:synchronized\s+)?(?:abstract\s+)?(?:\S+)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(([^)]*)\)",
    )
    .unwrap();

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        if let Some(caps) = re_class.captures(line) {
            let vis = if line.contains("public") {
                Visibility::Public
            } else {
                Visibility::Private
            };
            symbols.push(Symbol {
                name: caps[1].to_string(),
                kind: SymbolKind::Class,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        if let Some(caps) = re_interface.captures(line) {
            let vis = if line.contains("public") {
                Visibility::Public
            } else {
                Visibility::Private
            };
            symbols.push(Symbol {
                name: caps[1].to_string(),
                kind: SymbolKind::Interface,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: None,
            });
            continue;
        }

        if let Some(caps) = re_method.captures(line) {
            let vis = match caps.get(1).map(|m| m.as_str()) {
                Some("public") => Visibility::Public,
                _ => Visibility::Private,
            };
            symbols.push(Symbol {
                name: caps[2].to_string(),
                kind: SymbolKind::Method,
                visibility: vis,
                span: Span {
                    start_line: line_num,
                    end_line: line_num,
                },
                parent: None,
                signature: Some(format!("({})", &caps[3])),
            });
        }
    }
}

// ── Span computation ────────────────────────────────────────

/// Compute end_line for symbols by tracking brace/indent depth.
fn compute_spans(lines: &[&str], symbols: &mut [Symbol], lang: &Language) {
    match lang {
        Language::Python => compute_spans_indent(lines, symbols),
        _ => compute_spans_braces(lines, symbols),
    }
}

/// Brace-based span computation for C-family languages.
fn compute_spans_braces(lines: &[&str], symbols: &mut [Symbol]) {
    for sym in symbols.iter_mut() {
        let start_idx = sym.span.start_line.saturating_sub(1);
        let mut depth = 0i32;
        let mut found_open = false;

        for (i, line) in lines.iter().enumerate().skip(start_idx) {
            for ch in line.chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        found_open = true;
                    }
                    '}' => {
                        depth -= 1;
                    }
                    _ => {}
                }
            }
            if found_open && depth <= 0 {
                sym.span.end_line = i + 1;
                break;
            }
        }
        // If we never found a closing brace, keep end_line = start_line
    }
}

/// Indent-based span computation for Python.
fn compute_spans_indent(lines: &[&str], symbols: &mut [Symbol]) {
    for sym in symbols.iter_mut() {
        let start_idx = sym.span.start_line.saturating_sub(1);
        if start_idx >= lines.len() {
            continue;
        }

        // Determine base indent of the definition line
        let base_indent = lines[start_idx].len() - lines[start_idx].trim_start().len();

        let mut last_line = sym.span.start_line;
        for (i, line) in lines.iter().enumerate().skip(start_idx + 1) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue; // skip blank lines
            }
            let indent = line.len() - trimmed.len();
            if indent <= base_indent {
                break; // back to same or outer indent
            }
            last_line = i + 1;
        }
        sym.span.end_line = last_line;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_extracts_functions_and_structs() {
        let code = r#"
pub struct Config {
    name: String,
}

pub fn run(path: &Path) -> Result<()> {
    Ok(())
}

fn helper() {
}
"#;
        let ext = RegexExtractor;
        let syms = ext.extract(code, &Language::Rust);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Config"));
        assert!(names.contains(&"run"));
        assert!(names.contains(&"helper"));
    }

    #[test]
    fn rust_detects_visibility() {
        let code = "pub fn public_fn() {}\nfn private_fn() {}\n";
        let ext = RegexExtractor;
        let syms = ext.extract(code, &Language::Rust);
        let pub_fn = syms.iter().find(|s| s.name == "public_fn").unwrap();
        let priv_fn = syms.iter().find(|s| s.name == "private_fn").unwrap();
        assert_eq!(pub_fn.visibility, Visibility::Public);
        assert_eq!(priv_fn.visibility, Visibility::Private);
    }

    #[test]
    fn rust_trait_and_enum() {
        let code = "pub trait Handler {}\nenum State { A, B }\n";
        let ext = RegexExtractor;
        let syms = ext.extract(code, &Language::Rust);
        assert!(syms
            .iter()
            .any(|s| s.name == "Handler" && s.kind == SymbolKind::Trait));
        assert!(syms
            .iter()
            .any(|s| s.name == "State" && s.kind == SymbolKind::Enum));
    }

    #[test]
    fn rust_impl_methods_get_parent() {
        let code = r#"
impl Config {
    pub fn new() -> Self {
        Config {}
    }
    fn private_method(&self) {
    }
}
"#;
        let ext = RegexExtractor;
        let syms = ext.extract(code, &Language::Rust);
        let new_sym = syms.iter().find(|s| s.name == "new").unwrap();
        assert_eq!(new_sym.kind, SymbolKind::Method);
        assert_eq!(new_sym.parent, Some("Config".to_string()));
    }

    #[test]
    fn python_class_and_methods() {
        let code = r#"
class Config:
    def __init__(self, name):
        self.name = name

    def _private(self):
        pass

def standalone():
    pass
"#;
        let ext = RegexExtractor;
        let syms = ext.extract(code, &Language::Python);
        assert!(syms
            .iter()
            .any(|s| s.name == "Config" && s.kind == SymbolKind::Class));
        assert!(syms
            .iter()
            .any(|s| s.name == "__init__" && s.kind == SymbolKind::Method));
        assert!(syms
            .iter()
            .any(|s| s.name == "_private" && s.visibility == Visibility::Private));
        assert!(syms
            .iter()
            .any(|s| s.name == "standalone" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn typescript_extracts_interface_and_type() {
        let code = r#"
export interface User {
  id: string;
}

export type Config = {
  name: string;
};

export function run(args: string[]): void {
}

const helper = (x: number) => x + 1;
"#;
        let ext = RegexExtractor;
        let syms = ext.extract(code, &Language::TypeScript);
        assert!(syms
            .iter()
            .any(|s| s.name == "User" && s.kind == SymbolKind::Interface));
        assert!(syms
            .iter()
            .any(|s| s.name == "Config" && s.kind == SymbolKind::Type));
        assert!(syms
            .iter()
            .any(|s| s.name == "run" && s.kind == SymbolKind::Function));
        assert!(syms
            .iter()
            .any(|s| s.name == "helper" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn go_extracts_struct_and_methods() {
        let code = r#"
type Config struct {
    Name string
}

func (c *Config) Load() error {
    return nil
}

func main() {
}
"#;
        let ext = RegexExtractor;
        let syms = ext.extract(code, &Language::Go);
        assert!(syms
            .iter()
            .any(|s| s.name == "Config" && s.kind == SymbolKind::Struct));
        let load = syms.iter().find(|s| s.name == "Load").unwrap();
        assert_eq!(load.kind, SymbolKind::Method);
        assert_eq!(load.parent, Some("Config".to_string()));
        assert_eq!(load.visibility, Visibility::Public);
        let main_fn = syms.iter().find(|s| s.name == "main").unwrap();
        assert_eq!(main_fn.visibility, Visibility::Private);
    }

    #[test]
    fn brace_span_computation() {
        let code = "fn foo() {\n  let x = 1;\n  let y = 2;\n}\n";
        let ext = RegexExtractor;
        let syms = ext.extract(code, &Language::Rust);
        let foo = syms.iter().find(|s| s.name == "foo").unwrap();
        assert_eq!(foo.span.start_line, 1);
        assert_eq!(foo.span.end_line, 4);
    }

    #[test]
    fn python_indent_span() {
        let code = "def foo():\n    x = 1\n    y = 2\n\ndef bar():\n    pass\n";
        let ext = RegexExtractor;
        let syms = ext.extract(code, &Language::Python);
        let foo = syms.iter().find(|s| s.name == "foo").unwrap();
        assert_eq!(foo.span.start_line, 1);
        assert_eq!(foo.span.end_line, 3);
    }

    #[test]
    fn unsupported_language_returns_empty() {
        let ext = RegexExtractor;
        let syms = ext.extract("some code", &Language::Shell);
        assert!(syms.is_empty());
    }
}
