// E0.1: file analysis submodule (language detection, symbol extraction)
use super::{SymbolSummary, IMPORT_SCAN_MAX_BYTES};
use crate::filter::Language;
use crate::read_symbols::{SymbolExtractor, Visibility};
use crate::symbols_regex::RegexExtractor;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::fs;
use std::path::Path;

pub(super) struct FileAnalysis {
    pub(super) language: Option<String>,
    pub(super) line_count: Option<u32>,
    pub(super) imports: Vec<String>,
    pub(super) pub_symbols: Vec<SymbolSummary>, // L3: public API surface
    pub(super) type_relations: Vec<super::TypeRelation>, // L2: type graph edges
}

pub(super) fn analyze_file(path: &Path, size: u64, current_hash: u64) -> Result<FileAnalysis> {
    let language = detect_language(path);

    if size > IMPORT_SCAN_MAX_BYTES {
        return Ok(FileAnalysis {
            language,
            line_count: None,
            imports: Vec::new(),
            pub_symbols: Vec::new(),
            type_relations: Vec::new(),
        });
    }

    let content = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(_) => {
            return Ok(FileAnalysis {
                language,
                line_count: None,
                imports: Vec::new(),
                pub_symbols: Vec::new(),
                type_relations: Vec::new(),
            });
        }
    };

    let line_count = Some(content.lines().count() as u32);
    let mut imports = extract_imports(&content);
    imports.sort();
    imports.dedup();

    if imports.len() > 64 {
        imports.truncate(64);
    }

    // Include a synthetic hash anchor for downstream consumers.
    if imports.is_empty() {
        imports.push(format!("self:{:016x}", current_hash));
    }

    // L3: extract public symbols for API surface caching
    let pub_symbols = language
        .as_deref()
        .map(|lang| extract_file_symbols(&content, lang))
        .unwrap_or_default();

    // L2: extract type relationships for type_graph layer
    let rel_path = path.to_string_lossy().to_string();
    let type_relations = language
        .as_deref()
        .map(|lang| extract_type_relations(&content, lang, &rel_path))
        .unwrap_or_default();

    Ok(FileAnalysis {
        language,
        line_count,
        imports,
        pub_symbols,
        type_relations,
    })
}

pub(super) fn language_str_to_filter(lang: &str) -> Option<Language> {
    match lang {
        "rust" => Some(Language::Rust),
        "typescript" => Some(Language::TypeScript),
        "javascript" => Some(Language::JavaScript),
        "python" => Some(Language::Python),
        "go" => Some(Language::Go),
        _ => None,
    }
}

pub(super) fn symbol_kind_label(kind: crate::read_symbols::SymbolKind) -> &'static str {
    use crate::read_symbols::SymbolKind;
    match kind {
        SymbolKind::Function | SymbolKind::Method => "fn",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Interface => "iface",
        SymbolKind::Class => "class",
        SymbolKind::Type => "type",
        SymbolKind::Constant => "const",
        SymbolKind::Module => "mod",
        SymbolKind::Import => "import",
    }
}

/// Extract public symbols from file content and return compact SymbolSummary list.
pub(super) fn extract_file_symbols(content: &str, lang_str: &str) -> Vec<SymbolSummary> {
    let lang = match language_str_to_filter(lang_str) {
        Some(l) => l,
        None => return Vec::new(),
    };
    let extractor = RegexExtractor;
    extractor
        .extract(content, &lang)
        .into_iter()
        .filter(|s| s.visibility == Visibility::Public)
        .take(super::mem_config().max_symbols_per_file) // configurable via [mem] max_symbols_per_file
        .map(|s| SymbolSummary {
            kind: symbol_kind_label(s.kind).to_string(),
            name: s.name.clone(),
            sig: s.signature.clone(),
        })
        .collect()
}

pub(super) fn detect_language(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let language = match ext.as_str() {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "rb" => "ruby",
        "php" => "php",
        "scala" => "scala",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "cpp",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        _ => return None,
    };

    Some(language.to_string())
}

/// L2: Extract type relationships (implements, extends, contains, alias) from source code.
/// Regex-based — covers common patterns for Rust, TypeScript, Python, Go.
pub(super) fn extract_type_relations(
    content: &str,
    lang: &str,
    file_path: &str,
) -> Vec<super::TypeRelation> {
    lazy_static! {
        // Rust: impl Trait for Struct
        static ref RUST_IMPL_FOR: Regex =
            Regex::new(r"^\s*impl\s+(\w+)\s+for\s+(\w+)").expect("valid rust impl-for regex");
        // Rust: impl Struct (inherent impl — skip, no relation)
        // Rust: struct Foo { field: Bar, ... } — extract field types
        static ref RUST_STRUCT_FIELD: Regex =
            Regex::new(r"^\s*(?:pub(?:\([\w:]+\))?\s+)?(\w+)\s*:\s*(?:&(?:'[\w]+\s+)?(?:mut\s+)?)?(\w+)")
                .expect("valid rust struct field regex");
        // Rust: type Alias = Target
        static ref RUST_TYPE_ALIAS: Regex =
            Regex::new(r"^\s*(?:pub\s+)?type\s+(\w+)\s*=\s*(\w+)")
                .expect("valid rust type alias regex");
        // TypeScript/JavaScript: class Foo extends Bar implements Baz
        static ref TS_EXTENDS: Regex =
            Regex::new(r"^\s*(?:export\s+)?class\s+(\w+)\s+extends\s+(\w+)")
                .expect("valid ts extends regex");
        static ref TS_IMPLEMENTS: Regex =
            Regex::new(r"^\s*(?:export\s+)?class\s+(\w+)(?:\s+extends\s+\w+)?\s+implements\s+(\w+)")
                .expect("valid ts implements regex");
        // TypeScript: type Alias = Target
        static ref TS_TYPE_ALIAS: Regex =
            Regex::new(r"^\s*(?:export\s+)?type\s+(\w+)\s*=\s*(\w+)")
                .expect("valid ts type alias regex");
        // Python: class Foo(Bar, Baz):
        static ref PY_CLASS_BASES: Regex =
            Regex::new(r"^\s*class\s+(\w+)\s*\(([^)]+)\)\s*:")
                .expect("valid python class bases regex");
        // Go: type Foo struct { ... } — field types extracted separately
        // Go: type Foo interface { Bar; ... } — embedding
        static ref GO_TYPE_DECL: Regex =
            Regex::new(r"^\s*type\s+(\w+)\s+(struct|interface)")
                .expect("valid go type decl regex");
    }

    let mut relations = Vec::new();
    let primitives = [
        "bool", "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64", "u128", "f32", "f64",
        "usize", "isize", "str", "String", "char", "Vec", "Option", "Result", "Box", "Arc", "Rc",
        "HashMap", "HashSet", "BTreeMap", "BTreeSet", "Path", "PathBuf", "string", "number",
        "boolean", "any", "void", "int", "float", "None", "object",
    ];
    let is_primitive = |t: &str| primitives.contains(&t) || t.starts_with('_');

    // Track if we're inside a struct block (Rust)
    let mut in_struct: Option<String> = None;
    let mut brace_depth: i32 = 0;

    for line in content.lines() {
        match lang {
            "rust" => {
                // Track struct blocks for field extraction
                if let Some(ref struct_name) = in_struct {
                    brace_depth += line.matches('{').count() as i32;
                    brace_depth -= line.matches('}').count() as i32;
                    if brace_depth <= 0 {
                        in_struct = None;
                        brace_depth = 0;
                        continue;
                    }
                    // Extract field type
                    if let Some(cap) = RUST_STRUCT_FIELD.captures(line) {
                        let target = &cap[2];
                        if !is_primitive(target) {
                            relations.push(super::TypeRelation {
                                source: struct_name.clone(),
                                target: target.to_string(),
                                relation: "contains".to_string(),
                                file: file_path.to_string(),
                            });
                        }
                    }
                    continue;
                }

                if let Some(cap) = RUST_IMPL_FOR.captures(line) {
                    relations.push(super::TypeRelation {
                        source: cap[2].to_string(),
                        target: cap[1].to_string(),
                        relation: "implements".to_string(),
                        file: file_path.to_string(),
                    });
                }
                if let Some(cap) = RUST_TYPE_ALIAS.captures(line) {
                    let target = &cap[2];
                    if !is_primitive(target) {
                        relations.push(super::TypeRelation {
                            source: cap[1].to_string(),
                            target: target.to_string(),
                            relation: "alias".to_string(),
                            file: file_path.to_string(),
                        });
                    }
                }
                // Detect start of struct block
                if line.contains("struct ") && line.contains('{') {
                    if let Some(name) = line
                        .split("struct ")
                        .nth(1)
                        .and_then(|s| s.split_whitespace().next())
                        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()))
                    {
                        if !name.is_empty() {
                            in_struct = Some(name.to_string());
                            brace_depth =
                                line.matches('{').count() as i32 - line.matches('}').count() as i32;
                            if brace_depth <= 0 {
                                in_struct = None;
                            }
                        }
                    }
                }
            }
            "typescript" | "javascript" => {
                if let Some(cap) = TS_EXTENDS.captures(line) {
                    relations.push(super::TypeRelation {
                        source: cap[1].to_string(),
                        target: cap[2].to_string(),
                        relation: "extends".to_string(),
                        file: file_path.to_string(),
                    });
                }
                if let Some(cap) = TS_IMPLEMENTS.captures(line) {
                    relations.push(super::TypeRelation {
                        source: cap[1].to_string(),
                        target: cap[2].to_string(),
                        relation: "implements".to_string(),
                        file: file_path.to_string(),
                    });
                }
                if let Some(cap) = TS_TYPE_ALIAS.captures(line) {
                    let target = &cap[2];
                    if !is_primitive(target) {
                        relations.push(super::TypeRelation {
                            source: cap[1].to_string(),
                            target: target.to_string(),
                            relation: "alias".to_string(),
                            file: file_path.to_string(),
                        });
                    }
                }
            }
            "python" => {
                if let Some(cap) = PY_CLASS_BASES.captures(line) {
                    let class_name = &cap[1];
                    for base in cap[2].split(',') {
                        let base = base.trim();
                        if !base.is_empty() && !is_primitive(base) && base != "object" {
                            relations.push(super::TypeRelation {
                                source: class_name.to_string(),
                                target: base.to_string(),
                                relation: "extends".to_string(),
                                file: file_path.to_string(),
                            });
                        }
                    }
                }
            }
            "go" => {
                if let Some(cap) = GO_TYPE_DECL.captures(line) {
                    // For Go, we note the type declaration; field extraction requires multi-line
                    // which is out of scope for regex v1. Just record the type exists.
                    let _ = (&cap[1], &cap[2]); // suppress unused
                }
            }
            _ => {}
        }
    }

    // Cap to prevent bloat
    relations.truncate(128);
    relations
}

pub(super) fn extract_imports(content: &str) -> Vec<String> {
    lazy_static! {
        static ref JS_IMPORT_RE: Regex =
            Regex::new(r#"^\s*import\s+.+\s+from\s+['\"]([^'\"]+)['\"]"#)
                .expect("valid JS import regex");
        static ref JS_REQUIRE_RE: Regex =
            Regex::new(r#"require\(\s*['\"]([^'\"]+)['\"]\s*\)"#).expect("valid JS require regex");
        static ref PY_IMPORT_RE: Regex =
            Regex::new(r"^\s*import\s+([A-Za-z0-9_\.]+)").expect("valid Python import regex");
        static ref PY_FROM_RE: Regex = Regex::new(r"^\s*from\s+([A-Za-z0-9_\.]+)\s+import\s+")
            .expect("valid Python from-import regex");
        static ref RUST_USE_RE: Regex =
            Regex::new(r"^\s*use\s+([^;]+);").expect("valid Rust use regex");
        static ref GO_IMPORT_RE: Regex =
            Regex::new(r#"^\s*import\s+['\"]([^'\"]+)['\"]"#).expect("valid Go import regex");
    }

    let mut imports = Vec::new();

    for line in content.lines() {
        if let Some(cap) = JS_IMPORT_RE.captures(line) {
            imports.push(cap[1].trim().to_string());
        }

        if let Some(cap) = JS_REQUIRE_RE.captures(line) {
            imports.push(cap[1].trim().to_string());
        }

        if let Some(cap) = PY_IMPORT_RE.captures(line) {
            imports.push(cap[1].trim().to_string());
        }

        if let Some(cap) = PY_FROM_RE.captures(line) {
            imports.push(cap[1].trim().to_string());
        }

        if let Some(cap) = RUST_USE_RE.captures(line) {
            imports.push(cap[1].trim().to_string());
        }

        if let Some(cap) = GO_IMPORT_RE.captures(line) {
            imports.push(cap[1].trim().to_string());
        }
    }

    imports
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detect_language_by_extension() {
        assert_eq!(
            detect_language(Path::new("foo.rs")),
            Some("rust".to_string())
        );
        assert_eq!(
            detect_language(Path::new("foo.ts")),
            Some("typescript".to_string())
        );
        assert_eq!(
            detect_language(Path::new("foo.tsx")),
            Some("typescript".to_string())
        );
        assert_eq!(
            detect_language(Path::new("foo.py")),
            Some("python".to_string())
        );
        assert_eq!(detect_language(Path::new("foo.go")), Some("go".to_string()));
        assert_eq!(detect_language(Path::new("foo.xyz")), None);
    }
}
