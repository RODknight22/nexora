use std::ffi::OsStr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rayon::prelude::*;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;

use crate::safety;
use crate::scanner::paths::{
    extract_extension, join_display_path, join_extended_path, strip_extended_prefix,
    to_extended_length_path,
};
use crate::scanner::types::{ScanError, ScanNode, ScanOptions, ScanProgressEvent, ScanResult};
use crate::scanner::win_fs::{
    filetime_to_unix_ms, is_cloud_only_attributes, is_name_surrogate_reparse_point,
    is_symlink_fallback, query_size_on_disk, read_directory, DirReadOutcome, WinDirEntry,
};

const PROGRESS_THROTTLE_MS: u64 = 75;
const MAX_RECORDED_ERRORS: usize = 500;
const PARALLEL_THRESHOLD: usize = 4;

pub trait ProgressSink: Send + Sync {
    fn on_progress(&self, event: &ScanProgressEvent);
}

pub struct NoopProgressSink;

impl ProgressSink for NoopProgressSink {
    fn on_progress(&self, _event: &ScanProgressEvent) {}
}

impl<F> ProgressSink for F
where
    F: Fn(&ScanProgressEvent) + Send + Sync,
{
    fn on_progress(&self, event: &ScanProgressEvent) {
        self(event);
    }
}

struct ScanContext<'a> {
    scan_id: &'a str,
    cancel_token: &'a Arc<AtomicBool>,
    options: &'a ScanOptions,
    started_at: Instant,
    scanned_files: AtomicU64,
    scanned_dirs: AtomicU64,
    logical_bytes: AtomicU64,
    size_on_disk_bytes: AtomicU64,
    cloud_only_files: AtomicU64,
    error_count: AtomicU64,
    skipped_protected_count: AtomicU64,
    last_emit_ms: AtomicU64,
    errors: Mutex<Vec<ScanError>>,
    sink: &'a dyn ProgressSink,
}

impl<'a> ScanContext<'a> {
    fn is_cancelled(&self) -> bool {
        self.cancel_token.load(Ordering::Relaxed)
    }

    fn record_error(&self, path: &str, kind: &str, message: String) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut guard) = self.errors.lock() {
            if guard.len() < MAX_RECORDED_ERRORS {
                guard.push(ScanError {
                    path: path.to_string(),
                    kind: kind.to_string(),
                    message,
                });
            }
        }
    }

    fn maybe_emit_progress(&self, current_path: &str) {
        let elapsed_ms = self.started_at.elapsed().as_millis() as u64;
        let prev = self.last_emit_ms.load(Ordering::Relaxed);
        if elapsed_ms.saturating_sub(prev) < PROGRESS_THROTTLE_MS {
            return;
        }
        if self
            .last_emit_ms
            .compare_exchange(prev, elapsed_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            self.emit_now(current_path, false);
        }
    }

    fn emit_now(&self, current_path: &str, finished: bool) {
        let event = ScanProgressEvent {
            scan_id: self.scan_id.to_string(),
            scanned_files: self.scanned_files.load(Ordering::Relaxed),
            scanned_dirs: self.scanned_dirs.load(Ordering::Relaxed),
            logical_bytes: self.logical_bytes.load(Ordering::Relaxed),
            size_on_disk_bytes: self.size_on_disk_bytes.load(Ordering::Relaxed),
            cloud_only_files: self.cloud_only_files.load(Ordering::Relaxed),
            error_count: self.error_count.load(Ordering::Relaxed),
            current_path: current_path.to_string(),
            cancelled: self.is_cancelled(),
            finished,
        };
        self.sink.on_progress(&event);
    }
}

/// Classifies an `io::ErrorKind` into a consistent scanner error category.
pub fn classify_io_error_kind(kind: std::io::ErrorKind) -> &'static str {
    match kind {
        std::io::ErrorKind::PermissionDenied => "accessDenied",
        _ => "ioError",
    }
}

/// Runs a multi-threaded scan of `root_path` and returns a hierarchical `ScanResult`.
pub fn scan_path(
    scan_id: String,
    root_path: &Path,
    options: ScanOptions,
    cancel_token: Arc<AtomicBool>,
    sink: &dyn ProgressSink,
) -> Result<ScanResult, String> {
    let started_at = Instant::now();
    let ext_root = to_extended_length_path(root_path);
    let display_root = strip_extended_prefix(Path::new(&ext_root));

    let root_meta = std::fs::symlink_metadata(&ext_root)
        .map_err(|err| format!("No se puede acceder a '{display_root}': {err}"))?;

    let ctx = ScanContext {
        scan_id: &scan_id,
        cancel_token: &cancel_token,
        options: &options,
        started_at,
        scanned_files: AtomicU64::new(0),
        scanned_dirs: AtomicU64::new(0),
        logical_bytes: AtomicU64::new(0),
        size_on_disk_bytes: AtomicU64::new(0),
        cloud_only_files: AtomicU64::new(0),
        error_count: AtomicU64::new(0),
        skipped_protected_count: AtomicU64::new(0),
        last_emit_ms: AtomicU64::new(0),
        errors: Mutex::new(Vec::new()),
        sink,
    };

    ctx.emit_now(&display_root, false);

    use std::os::windows::fs::MetadataExt;
    let root_node = if root_meta.is_dir() {
        let root_is_drive = safety::is_drive_root(Path::new(&display_root));
        let root_is_critical = safety::is_critical_system_path(Path::new(&display_root));

        if root_is_critical && !root_is_drive && options.skip_protected {
            ctx.skipped_protected_count.fetch_add(1, Ordering::Relaxed);
            ctx.scanned_dirs.fetch_add(1, Ordering::Relaxed);
            make_protected_dir_marker(&display_root)
        } else {
            // Propagate protection down only when scanning inside a critical system subtree,
            // NOT when scanning from a drive root like C:\ (where C:\Users must remain deletable).
            let propagate_protected = root_is_critical && !root_is_drive;
            scan_directory(&ctx, &ext_root, &display_root, 0, propagate_protected, true)
        }
    } else if (root_meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY) != 0 {
        scan_root_reparse_dir(&ctx, &display_root, &root_meta)
    } else {
        scan_single_root_file(&ctx, &ext_root, &display_root, &root_meta)
    };

    let cancelled = ctx.is_cancelled();
    ctx.emit_now(&display_root, true);

    let duration_ms = started_at.elapsed().as_millis() as u64;
    let skipped_protected_count = ctx.skipped_protected_count.load(Ordering::Relaxed);
    let errors = ctx
        .errors
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    Ok(ScanResult {
        scan_id,
        root_path: display_root,
        logical_size: root_node.logical_size,
        size_on_disk: root_node.size_on_disk,
        file_count: root_node.file_count,
        dir_count: root_node.dir_count,
        cloud_only_count: root_node.cloud_only_count,
        cloud_only_logical_size: root_node.cloud_only_logical_size,
        error_count: root_node.error_count,
        skipped_protected_count,
        cancelled,
        duration_ms,
        root: root_node,
        errors,
    })
}

fn scan_directory(
    ctx: &ScanContext<'_>,
    ext_dir_path: &OsStr,
    display_path: &str,
    depth: usize,
    parent_protected: bool,
    is_root: bool,
) -> ScanNode {
    let dir_name = extract_node_name(display_path);
    let path_obj = Path::new(display_path);
    let is_critical = parent_protected || safety::is_critical_system_path(path_obj);
    let is_drive = safety::is_drive_root(path_obj);
    let is_protected = is_critical || is_drive;
    let is_deletable = !is_protected && safety::is_deletable_path(path_obj);

    ctx.scanned_dirs.fetch_add(1, Ordering::Relaxed);
    ctx.maybe_emit_progress(display_path);

    if ctx.is_cancelled() {
        return empty_dir_node(
            dir_name,
            display_path.to_string(),
            is_protected,
            is_deletable,
        );
    }

    if let Some(max_depth) = ctx.options.max_depth {
        if depth >= max_depth && !is_root {
            return empty_dir_node(
                dir_name,
                display_path.to_string(),
                is_protected,
                is_deletable,
            );
        }
    }

    let entries = match read_directory(ext_dir_path) {
        DirReadOutcome::Entries(items) => items,
        DirReadOutcome::AccessDenied(msg) => {
            ctx.record_error(display_path, "accessDenied", msg);
            let mut node = empty_dir_node(
                dir_name,
                display_path.to_string(),
                is_protected,
                is_deletable,
            );
            node.access_denied = true;
            node.error_count = 1;
            return node;
        }
        DirReadOutcome::Error(msg) => {
            ctx.record_error(display_path, "ioError", msg);
            let mut node = empty_dir_node(
                dir_name,
                display_path.to_string(),
                is_protected,
                is_deletable,
            );
            node.error_count = 1;
            return node;
        }
    };

    let child_nodes: Vec<(ScanNode, bool)> = if entries.len() > PARALLEL_THRESHOLD {
        entries
            .into_par_iter()
            .filter_map(|entry| {
                if ctx.is_cancelled() {
                    return None;
                }
                Some(process_entry(
                    ctx,
                    ext_dir_path,
                    display_path,
                    depth,
                    parent_protected,
                    entry,
                ))
            })
            .collect()
    } else {
        let mut collected = Vec::with_capacity(entries.len());
        for entry in entries {
            if ctx.is_cancelled() {
                break;
            }
            collected.push(process_entry(
                ctx,
                ext_dir_path,
                display_path,
                depth,
                parent_protected,
                entry,
            ));
        }
        collected
    };

    let mut logical_size: u64 = 0;
    let mut size_on_disk: u64 = 0;
    let mut file_count: u64 = 0;
    let mut dir_count: u64 = 0;
    let mut cloud_only_count: u64 = 0;
    let mut cloud_only_logical_size: u64 = 0;
    let mut error_count: u64 = 0;
    let mut children = Vec::new();

    for (child, keep_in_children) in child_nodes {
        logical_size = logical_size.saturating_add(child.logical_size);
        size_on_disk = size_on_disk.saturating_add(child.size_on_disk);
        file_count = file_count.saturating_add(child.file_count);
        if child.is_dir {
            dir_count = dir_count.saturating_add(1).saturating_add(child.dir_count);
        } else {
            dir_count = dir_count.saturating_add(child.dir_count);
        }
        cloud_only_count = cloud_only_count.saturating_add(child.cloud_only_count);
        cloud_only_logical_size =
            cloud_only_logical_size.saturating_add(child.cloud_only_logical_size);
        error_count = error_count.saturating_add(child.error_count);

        if keep_in_children {
            children.push(child);
        }
    }

    children.sort_by(|a, b| {
        b.size_on_disk
            .cmp(&a.size_on_disk)
            .then_with(|| b.logical_size.cmp(&a.logical_size))
            .then_with(|| a.name.cmp(&b.name))
    });

    ScanNode {
        name: dir_name,
        path: display_path.to_string(),
        is_dir: true,
        logical_size,
        size_on_disk,
        is_cloud_only: false,
        is_protected,
        is_deletable,
        skipped_protected: false,
        access_denied: false,
        is_reparse_point: false,
        extension: None,
        modified_ms: None,
        accessed_ms: None,
        file_count,
        dir_count,
        cloud_only_count,
        cloud_only_logical_size,
        error_count,
        children,
    }
}

fn process_entry(
    ctx: &ScanContext<'_>,
    parent_ext: &OsStr,
    parent_display: &str,
    depth: usize,
    parent_protected: bool,
    entry: WinDirEntry,
) -> (ScanNode, bool) {
    let child_display = join_display_path(parent_display, &entry.name);
    let child_ext = join_extended_path(parent_ext, &entry.name);
    let child_path_obj = Path::new(&child_display);
    let child_is_critical = parent_protected || safety::is_critical_system_path(child_path_obj);
    let child_is_protected = child_is_critical;
    let child_is_deletable = !child_is_protected && safety::is_deletable_path(child_path_obj);

    let is_surrogate_link = if is_name_surrogate_reparse_point(entry.attributes, entry.reparse_tag)
    {
        true
    } else if (entry.attributes
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT)
        != 0
        && entry.reparse_tag == 0
    {
        is_symlink_fallback(&child_ext)
    } else {
        false
    };

    if entry.is_dir {
        if is_surrogate_link {
            ctx.scanned_dirs.fetch_add(1, Ordering::Relaxed);
            let node = ScanNode {
                name: entry.name,
                path: child_display,
                is_dir: true,
                logical_size: 0,
                size_on_disk: 0,
                is_cloud_only: false,
                is_protected: child_is_protected,
                is_deletable: child_is_deletable,
                skipped_protected: false,
                access_denied: false,
                is_reparse_point: true,
                extension: None,
                modified_ms: entry.modified_ms,
                accessed_ms: entry.accessed_ms,
                file_count: 0,
                dir_count: 0,
                cloud_only_count: 0,
                cloud_only_logical_size: 0,
                error_count: 0,
                children: Vec::new(),
            };
            return (node, true);
        }

        if child_is_critical && ctx.options.skip_protected {
            ctx.skipped_protected_count.fetch_add(1, Ordering::Relaxed);
            ctx.scanned_dirs.fetch_add(1, Ordering::Relaxed);
            let mut marker = make_protected_dir_marker(&child_display);
            marker.modified_ms = entry.modified_ms;
            marker.accessed_ms = entry.accessed_ms;
            return (marker, true);
        }

        if let Some(max_depth) = ctx.options.max_depth {
            if depth + 1 >= max_depth {
                ctx.scanned_dirs.fetch_add(1, Ordering::Relaxed);
                let mut shallow = empty_dir_node(
                    entry.name,
                    child_display,
                    child_is_protected,
                    child_is_deletable,
                );
                shallow.modified_ms = entry.modified_ms;
                shallow.accessed_ms = entry.accessed_ms;
                return (shallow, true);
            }
        }

        let mut dir_node = scan_directory(
            ctx,
            &child_ext,
            &child_display,
            depth + 1,
            child_is_protected,
            false,
        );
        dir_node.modified_ms = entry.modified_ms;
        dir_node.accessed_ms = entry.accessed_ms;
        return (dir_node, true);
    }

    let is_cloud_only = is_cloud_only_attributes(entry.attributes);
    let mut access_denied = false;
    let mut error_count = 0u64;

    let size_on_disk = if is_cloud_only || is_surrogate_link {
        0
    } else {
        let outcome = query_size_on_disk(&child_ext, entry.logical_size);
        if outcome.access_denied {
            access_denied = true;
            error_count = 1;
            ctx.record_error(
                &child_display,
                "accessDenied",
                String::from("Access denied querying size on disk"),
            );
        }
        outcome.size_on_disk
    };

    ctx.scanned_files.fetch_add(1, Ordering::Relaxed);
    ctx.logical_bytes
        .fetch_add(entry.logical_size, Ordering::Relaxed);
    ctx.size_on_disk_bytes
        .fetch_add(size_on_disk, Ordering::Relaxed);
    if is_cloud_only {
        ctx.cloud_only_files.fetch_add(1, Ordering::Relaxed);
    }
    ctx.maybe_emit_progress(&child_display);

    let extension = extract_extension(&entry.name);
    let node = ScanNode {
        name: entry.name,
        path: child_display,
        is_dir: false,
        logical_size: entry.logical_size,
        size_on_disk,
        is_cloud_only,
        is_protected: child_is_protected,
        is_deletable: child_is_deletable,
        skipped_protected: false,
        access_denied,
        is_reparse_point: is_surrogate_link,
        extension,
        modified_ms: entry.modified_ms,
        accessed_ms: entry.accessed_ms,
        file_count: 1,
        dir_count: 0,
        cloud_only_count: u64::from(is_cloud_only),
        cloud_only_logical_size: if is_cloud_only { entry.logical_size } else { 0 },
        error_count,
        children: Vec::new(),
    };

    (node, ctx.options.include_files)
}

fn scan_root_reparse_dir(
    ctx: &ScanContext<'_>,
    display_path: &str,
    meta: &std::fs::Metadata,
) -> ScanNode {
    use std::os::windows::fs::MetadataExt;

    let attrs = meta.file_attributes();
    let is_reparse = meta.file_type().is_symlink()
        || (attrs & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT) != 0;
    let path_obj = Path::new(display_path);
    let is_protected = safety::is_critical_system_path(path_obj) || safety::is_drive_root(path_obj);
    let is_deletable = !is_protected && safety::is_deletable_path(path_obj);

    ctx.scanned_dirs.fetch_add(1, Ordering::Relaxed);

    ScanNode {
        name: extract_node_name(display_path),
        path: display_path.to_string(),
        is_dir: true,
        logical_size: 0,
        size_on_disk: 0,
        is_cloud_only: false,
        is_protected,
        is_deletable,
        skipped_protected: false,
        access_denied: false,
        is_reparse_point: is_reparse,
        extension: None,
        modified_ms: windows_time_to_ms(meta.last_write_time()),
        accessed_ms: windows_time_to_ms(meta.last_access_time()),
        file_count: 0,
        dir_count: 0,
        cloud_only_count: 0,
        cloud_only_logical_size: 0,
        error_count: 0,
        children: Vec::new(),
    }
}

fn scan_single_root_file(
    ctx: &ScanContext<'_>,
    ext_path: &OsStr,
    display_path: &str,
    meta: &std::fs::Metadata,
) -> ScanNode {
    use std::os::windows::fs::MetadataExt;

    let attrs = meta.file_attributes();
    let logical_size = meta.len();
    let is_cloud_only = is_cloud_only_attributes(attrs);
    let is_symlink = meta.file_type().is_symlink();
    let path_obj = Path::new(display_path);
    let is_protected = safety::is_critical_system_path(path_obj);
    let is_deletable = safety::is_deletable_path(path_obj);
    let mut access_denied = false;
    let mut error_count = 0u64;

    let size_on_disk = if is_cloud_only || is_symlink {
        0
    } else {
        let outcome = query_size_on_disk(ext_path, logical_size);
        if outcome.access_denied {
            access_denied = true;
            error_count = 1;
            ctx.record_error(
                display_path,
                "accessDenied",
                String::from("Access denied querying size on disk"),
            );
        }
        outcome.size_on_disk
    };

    ctx.scanned_files.fetch_add(1, Ordering::Relaxed);
    ctx.logical_bytes.fetch_add(logical_size, Ordering::Relaxed);
    ctx.size_on_disk_bytes
        .fetch_add(size_on_disk, Ordering::Relaxed);
    if is_cloud_only {
        ctx.cloud_only_files.fetch_add(1, Ordering::Relaxed);
    }

    let name = extract_node_name(display_path);
    let extension = extract_extension(&name);
    let modified_ms = windows_time_to_ms(meta.last_write_time());
    let accessed_ms = windows_time_to_ms(meta.last_access_time());

    ScanNode {
        name,
        path: display_path.to_string(),
        is_dir: false,
        logical_size,
        size_on_disk,
        is_cloud_only,
        is_protected,
        is_deletable,
        skipped_protected: false,
        access_denied,
        is_reparse_point: is_symlink,
        extension,
        modified_ms,
        accessed_ms,
        file_count: 1,
        dir_count: 0,
        cloud_only_count: u64::from(is_cloud_only),
        cloud_only_logical_size: if is_cloud_only { logical_size } else { 0 },
        error_count,
        children: Vec::new(),
    }
}

fn windows_time_to_ms(raw_100ns: u64) -> Option<u64> {
    let ft = windows_sys::Win32::Foundation::FILETIME {
        dwLowDateTime: raw_100ns as u32,
        dwHighDateTime: (raw_100ns >> 32) as u32,
    };
    filetime_to_unix_ms(ft)
}

fn make_protected_dir_marker(display_path: &str) -> ScanNode {
    ScanNode {
        name: extract_node_name(display_path),
        path: display_path.to_string(),
        is_dir: true,
        logical_size: 0,
        size_on_disk: 0,
        is_cloud_only: false,
        is_protected: true,
        is_deletable: false,
        skipped_protected: true,
        access_denied: false,
        is_reparse_point: false,
        extension: None,
        modified_ms: None,
        accessed_ms: None,
        file_count: 0,
        dir_count: 0,
        cloud_only_count: 0,
        cloud_only_logical_size: 0,
        error_count: 0,
        children: Vec::new(),
    }
}

fn empty_dir_node(name: String, path: String, is_protected: bool, is_deletable: bool) -> ScanNode {
    ScanNode {
        name,
        path,
        is_dir: true,
        logical_size: 0,
        size_on_disk: 0,
        is_cloud_only: false,
        is_protected,
        is_deletable,
        skipped_protected: false,
        access_denied: false,
        is_reparse_point: false,
        extension: None,
        modified_ms: None,
        accessed_ms: None,
        file_count: 0,
        dir_count: 0,
        cloud_only_count: 0,
        cloud_only_logical_size: 0,
        error_count: 0,
        children: Vec::new(),
    }
}

fn extract_node_name(display_path: &str) -> String {
    let trimmed = display_path.trim_end_matches(['\\', '/']);
    if trimmed.len() == 2 && trimmed.ends_with(':') {
        return format!("{trimmed}\\");
    }
    Path::new(trimmed)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| display_path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::AtomicUsize;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_OFFLINE,
    };
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    static NEXT_TEST_DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    struct TempTestDir {
        path: PathBuf,
        acl_protected: Vec<PathBuf>,
        junctions: Vec<PathBuf>,
    }

    impl TempTestDir {
        fn new(label: &str) -> Self {
            let seq = NEXT_TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let unique = format!("nexora_test_{label}_{pid}_{seq}");
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).expect("create isolated temp dir");
            Self {
                path,
                acl_protected: Vec::new(),
                junctions: Vec::new(),
            }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn register_acl_cleanup(&mut self, dir: PathBuf) {
            self.acl_protected.push(dir);
        }

        fn register_junction_cleanup(&mut self, junction: PathBuf) {
            self.junctions.push(junction);
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            for junction in self.junctions.iter().rev() {
                let _ = fs::remove_dir(junction);
            }
            for protected in self.acl_protected.iter().rev() {
                let _ = Command::new("icacls")
                    .arg(protected)
                    .args(["/remove:d", "*S-1-1-0"])
                    .output();
                let _ = Command::new("icacls")
                    .arg(protected)
                    .args(["/reset", "/T", "/Q"])
                    .output();
            }
            let ext = to_extended_length_path(&self.path);
            clear_readonly_recursive(Path::new(&ext));
            let _ = fs::remove_dir_all(&ext);
        }
    }

    fn clear_readonly_recursive(path: &Path) {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if let Ok(meta) = fs::symlink_metadata(&p) {
                    if meta.permissions().readonly() {
                        let wide = to_wide_null(&p);
                        unsafe {
                            SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_NORMAL);
                        }
                    }
                    if meta.is_dir() && !meta.file_type().is_symlink() {
                        clear_readonly_recursive(&p);
                    }
                }
            }
        }
    }

    fn write_bytes(path: &Path, size: usize) {
        let ext = to_extended_length_path(path);
        let mut f = File::create(&ext).expect("create temp file");
        let buf = vec![0x5Au8; size];
        f.write_all(&buf).expect("write temp file");
        f.sync_all().expect("sync temp file");
    }

    fn to_wide_null(path: &Path) -> Vec<u16> {
        to_extended_length_path(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    #[test]
    fn scans_normal_and_sparse_files_reporting_logical_and_size_on_disk() {
        let dir = TempTestDir::new("sparse_size");
        let normal_path = dir.path().join("normal.bin");
        write_bytes(&normal_path, 8192);

        let sparse_path = dir.path().join("sparse.bin");
        {
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .read(true)
                .truncate(true)
                .open(&sparse_path)
                .expect("create sparse candidate");
            let mut bytes_returned: u32 = 0;
            let ok = unsafe {
                DeviceIoControl(
                    file.as_raw_handle() as HANDLE,
                    FSCTL_SET_SPARSE,
                    std::ptr::null(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    &mut bytes_returned,
                    std::ptr::null_mut(),
                )
            };
            assert_ne!(ok, 0, "FSCTL_SET_SPARSE must succeed on NTFS");
            file.set_len(1_048_576).expect("set sparse logical length");
        }

        let result = scan_path(
            String::from("test-sparse"),
            dir.path(),
            ScanOptions::default(),
            Arc::new(AtomicBool::new(false)),
            &NoopProgressSink,
        )
        .expect("scan temp dir");

        assert!(!result.cancelled);
        assert_eq!(result.error_count, 0);
        assert_eq!(result.file_count, 2);
        assert_eq!(result.logical_size, 8192 + 1_048_576);
        assert_eq!(result.size_on_disk, 8192);

        let sparse_node = result
            .root
            .children
            .iter()
            .find(|n| n.name == "sparse.bin")
            .expect("sparse node present");
        assert_eq!(sparse_node.logical_size, 1_048_576);
        assert_eq!(sparse_node.size_on_disk, 0);
        assert!(!sparse_node.is_cloud_only);
        assert!(!sparse_node.is_protected);
        assert!(sparse_node.is_deletable);
    }

    #[test]
    fn identifies_cloud_only_offline_files_with_zero_size_on_disk() {
        let dir = TempTestDir::new("cloud_only");
        let cloud_path = dir.path().join("remote_photo.png");
        write_bytes(&cloud_path, 4096);

        let wide = to_wide_null(&cloud_path);
        let ok = unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_OFFLINE) };
        assert_ne!(ok, 0, "SetFileAttributesW(OFFLINE) should succeed");

        let result = scan_path(
            String::from("test-cloud"),
            dir.path(),
            ScanOptions::default(),
            Arc::new(AtomicBool::new(false)),
            &NoopProgressSink,
        )
        .expect("scan cloud temp dir");

        // Reset attribute before drop cleanup
        unsafe {
            SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_NORMAL);
        }

        assert_eq!(result.file_count, 1);
        assert_eq!(result.logical_size, 4096);
        assert_eq!(result.size_on_disk, 0);
        assert_eq!(result.cloud_only_count, 1);
        assert_eq!(result.cloud_only_logical_size, 4096);

        let node = &result.root.children[0];
        assert!(node.is_cloud_only);
        assert_eq!(node.size_on_disk, 0);
        assert_eq!(node.extension.as_deref(), Some("png"));
    }

    #[test]
    fn avoids_infinite_recursion_on_ntfs_directory_junction_loops() {
        let mut dir = TempTestDir::new("junction_loop");
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).expect("create sub dir");
        write_bytes(&dir.path().join("real.bin"), 1024);

        let loop_junction = sub.join("loop_back");
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&loop_junction)
            .arg(dir.path())
            .output()
            .expect("invoke mklink /J");
        assert!(
            status.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        dir.register_junction_cleanup(loop_junction);

        let result = scan_path(
            String::from("test-junction"),
            dir.path(),
            ScanOptions::default(),
            Arc::new(AtomicBool::new(false)),
            &NoopProgressSink,
        )
        .expect("scan junction dir");

        assert_eq!(result.error_count, 0);
        assert_eq!(result.file_count, 1);
        assert_eq!(result.logical_size, 1024);
        assert_eq!(result.size_on_disk, 1024);

        let sub_node = result
            .root
            .children
            .iter()
            .find(|n| n.name == "sub")
            .expect("sub node present");
        let link_node = sub_node
            .children
            .iter()
            .find(|n| n.name == "loop_back")
            .expect("junction node present");
        assert!(link_node.is_reparse_point);
        assert_eq!(link_node.size_on_disk, 0);
        assert!(link_node.children.is_empty());
    }

    #[test]
    fn scans_long_paths_beyond_280_chars_via_extended_prefix() {
        let dir = TempTestDir::new("long_path");
        let mut deep = dir.path().to_path_buf();
        for idx in 0..8 {
            deep = deep.join(format!("segment_{idx:02}_aaaaaaaaaaaaaaaaaaaaaaaa"));
        }
        let ext_deep = to_extended_length_path(&deep);
        fs::create_dir_all(&ext_deep).expect("create long path dirs");

        let deep_file = deep.join("deep_payload.bin");
        assert!(
            deep_file.to_string_lossy().len() > 280,
            "expected path > 280 chars, got {}",
            deep_file.to_string_lossy().len()
        );
        write_bytes(&deep_file, 2048);

        let result = scan_path(
            String::from("test-long-path"),
            dir.path(),
            ScanOptions::default(),
            Arc::new(AtomicBool::new(false)),
            &NoopProgressSink,
        )
        .expect("scan long path tree");

        assert_eq!(result.error_count, 0);
        assert_eq!(result.file_count, 1);
        assert_eq!(result.logical_size, 2048);
        assert_eq!(result.size_on_disk, 2048);
        assert!(!result.root_path.starts_with(r"\\?\"));
    }

    #[test]
    fn handles_access_denied_directories_without_crashing() {
        let mut dir = TempTestDir::new("access_denied");
        write_bytes(&dir.path().join("visible.bin"), 512);

        let locked = dir.path().join("locked_folder");
        fs::create_dir_all(&locked).expect("create locked folder");
        write_bytes(&locked.join("secret.bin"), 1024);

        let deny = Command::new("icacls")
            .arg(&locked)
            .args(["/deny", "*S-1-1-0:(OI)(CI)(RX,R)"])
            .output()
            .expect("invoke icacls deny");
        assert!(
            deny.status.success(),
            "icacls deny failed: {}",
            String::from_utf8_lossy(&deny.stderr)
        );
        dir.register_acl_cleanup(locked);

        let result = scan_path(
            String::from("test-acl"),
            dir.path(),
            ScanOptions::default(),
            Arc::new(AtomicBool::new(false)),
            &NoopProgressSink,
        )
        .expect("scan with access denied folder");

        assert!(result.error_count >= 1);
        assert!(result.logical_size >= 512);
        let locked_node = result
            .root
            .children
            .iter()
            .find(|n| n.name == "locked_folder")
            .expect("locked_folder node present");
        assert!(locked_node.access_denied);
        assert_eq!(locked_node.error_count, 1);

        assert_eq!(
            classify_io_error_kind(std::io::ErrorKind::PermissionDenied),
            "accessDenied"
        );
        assert_eq!(
            classify_io_error_kind(std::io::ErrorKind::NotFound),
            "ioError"
        );
    }

    #[test]
    fn aggregates_parallel_tree_emits_progress_and_supports_cancellation() {
        let dir = TempTestDir::new("parallel_progress");
        for sub_idx in 0..8 {
            let sub = dir.path().join(format!("bucket_{sub_idx}"));
            fs::create_dir_all(&sub).expect("create bucket");
            for file_idx in 0..10 {
                write_bytes(&sub.join(format!("file_{file_idx}.dat")), 256);
            }
        }

        let events: Mutex<Vec<ScanProgressEvent>> = Mutex::new(Vec::new());
        let result = scan_path(
            String::from("test-parallel"),
            dir.path(),
            ScanOptions::default(),
            Arc::new(AtomicBool::new(false)),
            &|ev: &ScanProgressEvent| {
                events.lock().expect("lock events").push(ev.clone());
            },
        )
        .expect("scan parallel tree");

        assert!(!result.cancelled);
        assert_eq!(result.file_count, 80);
        assert_eq!(result.dir_count, 8);
        assert_eq!(result.logical_size, 80 * 256);
        assert_eq!(result.size_on_disk, 80 * 256);

        let captured = events.into_inner().expect("unlock events");
        assert!(captured.len() >= 2, "expected start and completion events");
        assert!(!captured.first().expect("first event").finished);
        let last = captured.last().expect("last event");
        assert!(last.finished);
        assert!(!last.cancelled);
        assert_eq!(last.scanned_files, 80);

        // Immediate cancellation check
        let cancel_token = Arc::new(AtomicBool::new(false));
        let cancel_on_event = Arc::clone(&cancel_token);
        let cancelled_res = scan_path(
            String::from("test-cancel"),
            dir.path(),
            ScanOptions::default(),
            cancel_token,
            &move |_ev: &ScanProgressEvent| {
                cancel_on_event.store(true, Ordering::SeqCst);
            },
        )
        .expect("cancelled scan should return gracefully");

        assert!(cancelled_res.cancelled);
        assert!(cancelled_res.file_count < 80);
    }

    #[test]
    fn skips_critical_system_root_by_default_without_touching_system_files() {
        let cancel = Arc::new(AtomicBool::new(false));
        let res = scan_path(
            String::from("test-protected-skip"),
            Path::new(r"C:\Windows"),
            ScanOptions::default(),
            cancel,
            &NoopProgressSink,
        )
        .expect("protected skip succeeds");

        assert!(res.root.is_protected);
        assert!(!res.root.is_deletable);
        assert!(res.root.skipped_protected);
        assert_eq!(res.skipped_protected_count, 1);
        assert!(res.root.children.is_empty());
    }

    #[test]
    fn skips_critical_system_root_aliases_and_short_names_by_default() {
        let mut alias_paths = vec![
            r"\\.\C:\Windows",
            r"\??\C:\Windows",
            r"C:\Windows ",
            r"C:\Windows.",
            r"C:\Windows::$INDEX_ALLOCATION",
            r"\\.\C:\Windows::$INDEX_ALLOCATION",
            r"\\?\C:\Windows::$INDEX_ALLOCATION",
            r"C:\Program Files::$INDEX_ALLOCATION",
        ];
        if Path::new(r"C:\PROGRA~1").exists() {
            alias_paths.push(r"C:\PROGRA~1");
            alias_paths.push(r"C:\PROGRA~1::$INDEX_ALLOCATION");
        }

        for raw in alias_paths {
            let res = scan_path(
                String::from("test-protected-alias"),
                Path::new(raw),
                ScanOptions::default(),
                Arc::new(AtomicBool::new(false)),
                &NoopProgressSink,
            )
            .unwrap_or_else(|e| panic!("protected skip for '{raw}' failed: {e}"));

            assert!(res.root.is_protected, "expected isProtected for {raw}");
            assert!(!res.root.is_deletable, "expected !isDeletable for {raw}");
            assert!(
                res.root.skipped_protected,
                "expected skippedProtected for {raw}"
            );
            assert_eq!(res.skipped_protected_count, 1);
            assert!(res.root.children.is_empty());
        }
    }

    #[test]
    fn scans_non_ascii_prefixed_paths_without_panicking() {
        let dir = TempTestDir::new("unicode_prefix");
        let arbol = dir.path().join("Árbol");
        fs::create_dir_all(&arbol).expect("create unicode subdir");
        write_bytes(&arbol.join("hoja.txt"), 512);

        let prefixed = format!(r"\\?\{}", dir.path().display());
        let res = scan_path(
            String::from("test-unicode-prefix"),
            Path::new(&prefixed),
            ScanOptions::default(),
            Arc::new(AtomicBool::new(false)),
            &NoopProgressSink,
        )
        .expect("scan unicode prefixed temp dir");

        assert_eq!(res.error_count, 0);
        assert_eq!(res.file_count, 1);
        assert_eq!(res.logical_size, 512);
    }

    #[test]
    fn classifies_directory_junction_as_root_path_without_traversing() {
        let mut dir = TempTestDir::new("root_junction");
        let target = dir.path().join("target_dir");
        fs::create_dir_all(&target).expect("create target dir");
        write_bytes(&target.join("payload.bin"), 4096);

        let root_junction = dir.path().join("root_junc");
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&root_junction)
            .arg(&target)
            .output()
            .expect("invoke mklink /J");
        assert!(
            status.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        dir.register_junction_cleanup(root_junction.clone());

        let res = scan_path(
            String::from("test-root-junction"),
            &root_junction,
            ScanOptions::default(),
            Arc::new(AtomicBool::new(false)),
            &NoopProgressSink,
        )
        .expect("scan root junction");

        assert_eq!(res.error_count, 0);
        assert!(res.root.is_dir);
        assert!(res.root.is_reparse_point);
        assert_eq!(res.file_count, 0);
        assert_eq!(res.dir_count, 0);
        assert_eq!(res.logical_size, 0);
        assert_eq!(res.size_on_disk, 0);
        assert!(res.root.children.is_empty());
    }

    #[test]
    fn scan_result_and_progress_serialize_to_camel_case_json() {
        let dir = TempTestDir::new("json_contract");
        write_bytes(&dir.path().join("sample.exe"), 128);

        let res = scan_path(
            String::from("test-json"),
            dir.path(),
            ScanOptions::default(),
            Arc::new(AtomicBool::new(false)),
            &NoopProgressSink,
        )
        .expect("scan json test dir");

        let json = serde_json::to_value(&res).expect("serialize ScanResult");
        assert_eq!(json["scanId"], "test-json");
        assert_eq!(json["fileCount"], 1);
        assert_eq!(json["logicalSize"], 128);
        assert_eq!(json["sizeOnDisk"], 128);
        assert_eq!(json["root"]["children"][0]["extension"], "exe");
        assert_eq!(json["root"]["children"][0]["isDeletable"], true);
        assert_eq!(json["root"]["children"][0]["isProtected"], false);
    }

    #[test]
    fn skips_volume_guid_device_and_local_unc_critical_paths_by_default() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            GetVolumeNameForVolumeMountPointW, QueryDosDeviceW,
        };

        let c_root_w: Vec<u16> = std::ffi::OsStr::new(r"C:\")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut vol_buf = [0u16; 128];
        let ok = unsafe {
            GetVolumeNameForVolumeMountPointW(
                c_root_w.as_ptr(),
                vol_buf.as_mut_ptr(),
                vol_buf.len() as u32,
            )
        };
        assert_ne!(
            ok, 0,
            "GetVolumeNameForVolumeMountPointW(C:\\) must succeed"
        );
        let vol_len = vol_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(vol_buf.len());
        let vol_root = String::from_utf16_lossy(&vol_buf[..vol_len]);

        let c_dev_w: Vec<u16> = std::ffi::OsStr::new("C:")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut dev_buf = [0u16; 512];
        let dev_len = unsafe {
            QueryDosDeviceW(c_dev_w.as_ptr(), dev_buf.as_mut_ptr(), dev_buf.len() as u32)
        };
        assert_ne!(dev_len, 0, "QueryDosDeviceW(C:) must succeed");
        let dev_end = dev_buf[..dev_len as usize]
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(dev_len as usize);
        let nt_dev = String::from_utf16_lossy(&dev_buf[..dev_end]);

        let mut protected_targets = vec![
            format!("{vol_root}Windows"),
            format!("{vol_root}Program Files"),
            format!(r"\\.\GLOBALROOT{nt_dev}\Windows"),
            format!(r"\\?\GLOBALROOT{nt_dev}\Windows"),
            format!(r"\??\GLOBALROOT{nt_dev}\Windows"),
            String::from(r"\\.\BootPartition\Windows"),
            String::from(r"\\0--1.ipv6-literal.net\C$\Windows"),
        ];

        if let Ok(comp) = std::env::var("COMPUTERNAME") {
            let clean = comp.trim();
            if !clean.is_empty() {
                protected_targets.push(format!(r"\\{clean}\C$\Windows"));
                protected_targets.push(format!(r"\\?\UNC\{clean}\C$\Windows"));
            }
        }

        let mut tested = 0usize;
        for target in protected_targets {
            if std::fs::symlink_metadata(&target).is_err() {
                continue;
            }
            tested += 1;
            let res = scan_path(
                String::from("test-protected-namespace"),
                Path::new(&target),
                ScanOptions::default(),
                Arc::new(AtomicBool::new(false)),
                &NoopProgressSink,
            )
            .unwrap_or_else(|e| panic!("protected skip for '{target}' failed: {e}"));

            assert!(res.root.is_protected, "expected isProtected for {target}");
            assert!(!res.root.is_deletable, "expected !isDeletable for {target}");
            assert!(
                res.root.skipped_protected,
                "expected skippedProtected for {target}"
            );
            assert_eq!(res.skipped_protected_count, 1);
            assert!(res.root.children.is_empty());
        }
        assert!(
            tested >= 5,
            "expected at least 5 live namespace targets to be verified, got {tested}"
        );
    }
}
