use std::path::PathBuf;

pub const ICM_VERSION: &str = "0.10.61";
pub const ICM_TAG: &str = "icm-v0.10.61";
pub const ICM_COMMIT: &str = "c3a1bac7cfe401b55fd66af16dfc0c774c02167a";
pub const ICM_MCP_SERVER_VERSION: &str = "0.10.34";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcmInstallation {
    pub executable: PathBuf,
    pub version: String,
    pub sha256: Option<String>,
}
