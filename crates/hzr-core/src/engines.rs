use serde::{Deserialize, Serialize};

const LOCKED_ENGINES: &str = include_str!("../../../engines.lock.toml");

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EngineManifest {
    pub schema_version: u16,
    pub engine: Vec<EnginePin>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EnginePin {
    pub name: String,
    pub version: String,
    pub tag: String,
    pub commit: String,
    pub repository: String,
    pub license: String,
    pub binary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patch_sha256: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_sha256: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_schema: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_manifest_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_diff_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_status_sha256: Option<String>,
}

pub fn locked_engines() -> Result<EngineManifest, toml::de::Error> {
    toml::from_str(LOCKED_ENGINES)
}

#[cfg(test)]
mod tests {
    use super::locked_engines;

    #[test]
    fn test_engine_lock_contains_current_grepai() {
        let manifest = locked_engines().expect("engine lock parses");
        let grepai = manifest
            .engine
            .iter()
            .find(|engine| engine.name == "grepai")
            .expect("grepai pin");

        assert_eq!(grepai.version, "0.35.0");
        assert_eq!(grepai.tag, "v0.35.0");
        assert_eq!(
            grepai.patches,
            ["patches/grepai/0.35.0-disable-worktree-discovery.patch"]
        );
        assert_eq!(
            grepai.patch_sha256,
            ["55535352bc9f4837198c652b8c44ec54a0a7ef82fbd81e11b4ec11f4c4082991"]
        );

        let icm = manifest
            .engine
            .iter()
            .find(|engine| engine.name == "icm")
            .expect("ICM pin");
        assert_eq!(
            icm.patch_sha256,
            ["cd38e20e32f352bfde93a4ce297799ef8b5f984f8af928409ef0f3e47102e586"]
        );

        let caveman_code = manifest
            .engine
            .iter()
            .find(|engine| engine.name == "caveman-code")
            .expect("caveman-code pin");
        assert_eq!(caveman_code.version, "0.65.2");
        assert_eq!(
            caveman_code.commit,
            "4700b8fad23e45cedbb1a850f03ee9e2d4d49116"
        );
        assert!(caveman_code.integrity.is_some());

        let node = manifest
            .engine
            .iter()
            .find(|engine| engine.name == "nodejs")
            .expect("Node.js runtime pin");
        assert_eq!(node.version, "22.17.1");
        assert_eq!(node.binary, "node");
        assert_eq!(node.runtime, Some(true));
        assert_eq!(node.artifacts.len(), 4);
        assert_eq!(node.artifacts.len(), node.artifact_sha256.len());

        let fork = manifest
            .engine
            .iter()
            .find(|engine| engine.name == "rtk")
            .expect("fork-core pin");
        assert_eq!(fork.runtime, Some(true));
        assert_eq!(
            fork.source_kind.as_deref(),
            Some("immutable-worktree-snapshot")
        );
        assert_eq!(fork.snapshot_schema, Some(2));
        assert_eq!(
            fork.snapshot_sha256.as_deref(),
            Some("f4296ec404f461d6fc03c966c0dc79caee6c3118a73d1ed1a078ded5529f0a16")
        );
    }
}
