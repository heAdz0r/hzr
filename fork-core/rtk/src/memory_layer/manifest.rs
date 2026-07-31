// E0.1: dependency manifest parsing submodule
use super::{DepEntry, DepManifest};
use std::fs;
use std::path::Path;

/// L4: Parse dependency manifest from project root. Tries Cargo.toml → package.json → pyproject.toml.
pub(super) fn parse_dep_manifest(project_root: &Path) -> Option<DepManifest> {
    let cargo = project_root.join("Cargo.toml");
    if cargo.exists() {
        if let Ok(content) = fs::read_to_string(&cargo) {
            if let Some(m) = parse_cargo_toml_content(&content) {
                return Some(m);
            }
        }
    }
    let pkg = project_root.join("package.json");
    if pkg.exists() {
        if let Ok(content) = fs::read_to_string(&pkg) {
            if let Some(m) = parse_package_json_content(&content) {
                return Some(m);
            }
        }
    }
    let pyproject = project_root.join("pyproject.toml");
    if pyproject.exists() {
        if let Ok(content) = fs::read_to_string(&pyproject) {
            if let Some(m) = parse_pyproject_toml_content(&content) {
                return Some(m);
            }
        }
    }
    None
}

/// L4: Parse Cargo.toml content into DepManifest.
pub(super) fn parse_cargo_toml_content(content: &str) -> Option<DepManifest> {
    let table: toml::Value = toml::from_str(content).ok()?;
    let extract = |key: &str| -> Vec<DepEntry> {
        table
            .get(key)
            .and_then(|d| d.as_table())
            .map(|t| {
                t.iter()
                    .map(|(name, val)| {
                        let version = match val {
                            toml::Value::String(v) => v.clone(),
                            toml::Value::Table(t) => t
                                .get("version")
                                .and_then(|v| v.as_str())
                                .unwrap_or("*")
                                .to_string(),
                            _ => "*".to_string(),
                        };
                        DepEntry {
                            name: name.clone(),
                            version,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(DepManifest {
        runtime: extract("dependencies"),
        dev: extract("dev-dependencies"),
        build: extract("build-dependencies"),
    })
}

/// L4: Parse package.json content into DepManifest.
pub(super) fn parse_package_json_content(content: &str) -> Option<DepManifest> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    let extract = |key: &str| -> Vec<DepEntry> {
        json.get(key)
            .and_then(|d| d.as_object())
            .map(|m| {
                m.iter()
                    .map(|(name, ver)| DepEntry {
                        name: name.clone(),
                        version: ver.as_str().unwrap_or("*").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(DepManifest {
        runtime: extract("dependencies"),
        dev: extract("devDependencies"),
        build: vec![],
    })
}

/// L4: Parse pyproject.toml content into DepManifest.
pub(super) fn parse_pyproject_toml_content(content: &str) -> Option<DepManifest> {
    let table: toml::Value = toml::from_str(content).ok()?;
    let runtime: Vec<DepEntry> = table
        .get("project")
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| {
                    let (name, version) = split_pep508(s);
                    DepEntry {
                        name: name.to_string(),
                        version: version.to_string(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Some(DepManifest {
        runtime,
        dev: vec![],
        build: vec![],
    })
}

/// Split a PEP 508 dependency specifier (e.g. "requests>=2.28") into (name, constraint).
pub(super) fn split_pep508(spec: &str) -> (&str, &str) {
    let operators = [">=", "<=", "==", "!=", "~=", ">", "<", "["];
    let pos = operators.iter().filter_map(|op| spec.find(op)).min();
    match pos {
        Some(i) => (spec[..i].trim(), spec[i..].trim()),
        None => (spec.trim(), "*"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cargo_toml_content_extracts_deps() {
        let toml = r#"
[dependencies]
serde = { version = "1.0", features = ["derive"] }
anyhow = "1.0"

[dev-dependencies]
tempfile = "3"

[build-dependencies]
cc = "1"
"#;
        let manifest = parse_cargo_toml_content(toml).expect("valid Cargo.toml");
        assert!(manifest.runtime.iter().any(|d| d.name == "serde"));
        assert!(manifest.runtime.iter().any(|d| d.name == "anyhow"));
        assert!(manifest.dev.iter().any(|d| d.name == "tempfile"));
        assert!(manifest.build.iter().any(|d| d.name == "cc"));
        assert_eq!(
            manifest
                .runtime
                .iter()
                .find(|d| d.name == "anyhow")
                .map(|d| d.version.as_str()),
            Some("1.0")
        );
    }

    #[test]
    fn parse_package_json_content_extracts_deps() {
        let json = r#"{
  "dependencies": {
    "react": "^18.0.0",
    "express": "4.18.0"
  },
  "devDependencies": {
    "typescript": "5.0.0"
  }
}"#;
        let manifest = parse_package_json_content(json).expect("valid package.json");
        assert!(manifest.runtime.iter().any(|d| d.name == "react"));
        assert!(manifest.runtime.iter().any(|d| d.name == "express"));
        assert!(manifest.dev.iter().any(|d| d.name == "typescript"));
        assert!(manifest.build.is_empty());
    }

    #[test]
    fn parse_pyproject_toml_content_extracts_deps() {
        let toml = r#"
[project]
name = "myapp"
dependencies = ["requests>=2.28", "flask==2.0.0", "numpy"]
"#;
        let manifest = parse_pyproject_toml_content(toml).expect("valid pyproject.toml");
        assert!(manifest.runtime.iter().any(|d| d.name == "requests"));
        assert!(manifest.runtime.iter().any(|d| d.name == "flask"));
        assert!(manifest.runtime.iter().any(|d| d.name == "numpy"));
        let req = manifest
            .runtime
            .iter()
            .find(|d| d.name == "requests")
            .unwrap();
        assert_eq!(req.version, ">=2.28");
        let np = manifest.runtime.iter().find(|d| d.name == "numpy").unwrap();
        assert_eq!(np.version, "*");
    }

    #[test]
    fn split_pep508_handles_various_operators() {
        assert_eq!(split_pep508("requests>=2.28"), ("requests", ">=2.28"));
        assert_eq!(split_pep508("flask==2.0.0"), ("flask", "==2.0.0"));
        assert_eq!(split_pep508("numpy"), ("numpy", "*"));
        assert_eq!(split_pep508("pandas[excel]"), ("pandas", "[excel]"));
        assert_eq!(split_pep508("  scipy  "), ("scipy", "*"));
    }
}
