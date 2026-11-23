use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    pub letter: String,
    pub root_path: String,
    pub label: String,
    pub file_system: String,
    pub drive_type: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub is_ready: bool,
    pub is_protected: bool,
    pub is_deletable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UserFolderKind {
    Downloads,
    Desktop,
    Pictures,
    Documents,
    Videos,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserFolderInfo {
    pub kind: UserFolderKind,
    pub name: String,
    pub path: String,
    pub exists: bool,
    pub is_protected: bool,
    pub is_deletable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanTargets {
    pub drives: Vec<DriveInfo>,
    pub user_folders: Vec<UserFolderInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct ScanOptions {
    pub max_depth: Option<usize>,
    pub skip_protected: bool,
    pub include_files: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_depth: None,
            skip_protected: true,
            include_files: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRequest {
    pub scan_id: Option<String>,
    pub path: String,
    pub options: Option<ScanOptions>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub logical_size: u64,
    pub size_on_disk: u64,
    pub is_cloud_only: bool,
    pub is_protected: bool,
    pub is_deletable: bool,
    pub skipped_protected: bool,
    pub access_denied: bool,
    pub is_reparse_point: bool,
    pub extension: Option<String>,
    pub modified_ms: Option<u64>,
    pub accessed_ms: Option<u64>,
    pub file_count: u64,
    pub dir_count: u64,
    pub cloud_only_count: u64,
    pub cloud_only_logical_size: u64,
    pub error_count: u64,
    pub children: Vec<ScanNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    pub path: String,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub scan_id: String,
    pub root_path: String,
    pub logical_size: u64,
    pub size_on_disk: u64,
    pub file_count: u64,
    pub dir_count: u64,
    pub cloud_only_count: u64,
    pub cloud_only_logical_size: u64,
    pub error_count: u64,
    pub skipped_protected_count: u64,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub root: ScanNode,
    pub errors: Vec<ScanError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgressEvent {
    pub scan_id: String,
    pub scanned_files: u64,
    pub scanned_dirs: u64,
    pub logical_bytes: u64,
    pub size_on_disk_bytes: u64,
    pub cloud_only_files: u64,
    pub error_count: u64,
    pub current_path: String,
    pub cancelled: bool,
    pub finished: bool,
}
