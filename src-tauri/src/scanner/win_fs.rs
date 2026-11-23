use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    GetLastError, SetLastError, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_NOT_READY,
    ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION, FILETIME, HANDLE,
    INVALID_HANDLE_VALUE, NO_ERROR,
};
use windows_sys::Win32::Storage::FileSystem::{
    FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
    GetCompressedFileSizeW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
    GetVolumeInformationW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_PINNED,
    FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_UNPINNED, FIND_FIRST_EX_LARGE_FETCH,
    INVALID_FILE_SIZE, WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::SystemServices::{
    IO_REPARSE_TAG_CLOUD, IO_REPARSE_TAG_CLOUD_MASK, IO_REPARSE_TAG_MOUNT_POINT,
    IO_REPARSE_TAG_SYMLINK,
};
use windows_sys::Win32::System::WindowsProgramming::{DRIVE_FIXED, DRIVE_REMOVABLE};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Pictures, FOLDERID_Videos,
    SHGetKnownFolderPath,
};

use crate::safety;
use crate::scanner::paths::{strip_extended_prefix_str, to_utf16_null};
use crate::scanner::types::{DriveInfo, UserFolderInfo, UserFolderKind};

const IO_REPARSE_TAG_NAME_SURROGATE_BIT: u32 = 0x2000_0000;
const WINDOWS_TO_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;

#[derive(Debug, Clone)]
pub struct WinDirEntry {
    pub name: String,
    pub attributes: u32,
    pub reparse_tag: u32,
    pub is_dir: bool,
    pub logical_size: u64,
    pub modified_ms: Option<u64>,
    pub accessed_ms: Option<u64>,
}

#[derive(Debug)]
pub enum DirReadOutcome {
    Entries(Vec<WinDirEntry>),
    AccessDenied(String),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeOnDiskOutcome {
    pub size_on_disk: u64,
    pub access_denied: bool,
    pub error_code: Option<u32>,
}

struct FindHandleGuard(HANDLE);

impl Drop for FindHandleGuard {
    fn drop(&mut self) {
        if self.0 != INVALID_HANDLE_VALUE && !self.0.is_null() {
            unsafe {
                FindClose(self.0);
            }
        }
    }
}

/// Classifies whether a Win32 `dwFileAttributes` bitmask represents a cloud-only (dehydrated)
/// file without opening or hydrating the file contents.
///
/// Note: `FILE_ATTRIBUTE_UNPINNED` alone only expresses user intent ("free up space") and a file
/// may still have locally cached bytes until dehydrated; hydration absence is indicated by
/// `OFFLINE | RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN`.
pub fn is_cloud_only_attributes(attrs: u32) -> bool {
    let _pinned_or_unpinned = FILE_ATTRIBUTE_PINNED | FILE_ATTRIBUTE_UNPINNED;
    const CLOUD_ONLY_MASK: u32 = FILE_ATTRIBUTE_OFFLINE
        | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
        | FILE_ATTRIBUTE_RECALL_ON_OPEN;
    (attrs & CLOUD_ONLY_MASK) != 0
}

/// Returns true if the reparse tag belongs to Windows Cloud Filters (`IO_REPARSE_TAG_CLOUD*`, OneDrive).
pub fn is_cloud_reparse_tag(reparse_tag: u32) -> bool {
    reparse_tag != 0 && (reparse_tag & !IO_REPARSE_TAG_CLOUD_MASK) == IO_REPARSE_TAG_CLOUD
}

/// Returns true if the entry is a symlink or directory junction (name-surrogate reparse point)
/// that must NOT be followed during traversal or `GetCompressedFileSizeW` queries.
/// Cloud-filter directories (`IO_REPARSE_TAG_CLOUD*`) return `false` so OneDrive folders are traversed.
pub fn is_name_surrogate_reparse_point(attrs: u32, reparse_tag: u32) -> bool {
    if (attrs & FILE_ATTRIBUTE_REPARSE_POINT) == 0 {
        return false;
    }
    if is_cloud_reparse_tag(reparse_tag) {
        return false;
    }
    reparse_tag == IO_REPARSE_TAG_SYMLINK
        || reparse_tag == IO_REPARSE_TAG_MOUNT_POINT
        || (reparse_tag & IO_REPARSE_TAG_NAME_SURROGATE_BIT) != 0
}

/// Converts a Windows `FILETIME` to Unix epoch milliseconds.
pub fn filetime_to_unix_ms(ft: FILETIME) -> Option<u64> {
    let ticks = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
    if ticks <= WINDOWS_TO_UNIX_EPOCH_100NS {
        None
    } else {
        Some((ticks - WINDOWS_TO_UNIX_EPOCH_100NS) / 10_000)
    }
}

/// Enumerates all direct children of `ext_dir_path` (expects `\\?\`-prefixed path) using
/// `FindFirstFileExW` with `FindExInfoBasic` and `FIND_FIRST_EX_LARGE_FETCH`.
pub fn read_directory(ext_dir_path: &OsStr) -> DirReadOutcome {
    let raw = ext_dir_path.to_string_lossy();
    let pattern = if raw.ends_with('\\') {
        OsString::from(format!("{raw}*"))
    } else {
        OsString::from(format!("{raw}\\*"))
    };
    let pattern_w = to_utf16_null(&pattern);

    let mut find_data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let handle = unsafe {
        FindFirstFileExW(
            pattern_w.as_ptr(),
            FindExInfoBasic,
            (&mut find_data as *mut WIN32_FIND_DATAW).cast(),
            FindExSearchNameMatch,
            null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        let err = unsafe { GetLastError() };
        return match err {
            ERROR_FILE_NOT_FOUND | ERROR_NO_MORE_FILES => DirReadOutcome::Entries(Vec::new()),
            ERROR_ACCESS_DENIED => DirReadOutcome::AccessDenied(
                std::io::Error::from_raw_os_error(err as i32).to_string(),
            ),
            _ => DirReadOutcome::Error(std::io::Error::from_raw_os_error(err as i32).to_string()),
        };
    }

    let _guard = FindHandleGuard(handle);
    let mut entries = Vec::new();

    loop {
        let name_len = find_data
            .cFileName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(find_data.cFileName.len());
        let name = String::from_utf16_lossy(&find_data.cFileName[..name_len]);

        if name != "." && name != ".." {
            let attrs = find_data.dwFileAttributes;
            let is_dir = (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0;
            let logical_size = if is_dir {
                0
            } else {
                ((find_data.nFileSizeHigh as u64) << 32) | (find_data.nFileSizeLow as u64)
            };
            let reparse_tag = if (attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
                find_data.dwReserved0
            } else {
                0
            };

            entries.push(WinDirEntry {
                name,
                attributes: attrs,
                reparse_tag,
                is_dir,
                logical_size,
                modified_ms: filetime_to_unix_ms(find_data.ftLastWriteTime),
                accessed_ms: filetime_to_unix_ms(find_data.ftLastAccessTime),
            });
        }

        let has_next = unsafe { FindNextFileW(handle, &mut find_data) };
        if has_next == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NO_MORE_FILES {
                break;
            }
            if err == ERROR_ACCESS_DENIED {
                return DirReadOutcome::AccessDenied(
                    std::io::Error::from_raw_os_error(err as i32).to_string(),
                );
            }
            return DirReadOutcome::Error(
                std::io::Error::from_raw_os_error(err as i32).to_string(),
            );
        }
    }

    DirReadOutcome::Entries(entries)
}

/// Queries the real physical size on disk using `GetCompressedFileSizeW` on a `\\?\`-prefixed path.
/// Caller must have already excluded cloud-only files and symlinks/reparse surrogates.
pub fn query_size_on_disk(ext_file_path: &OsStr, logical_size: u64) -> SizeOnDiskOutcome {
    let path_w = to_utf16_null(ext_file_path);
    let mut high: u32 = 0;

    unsafe {
        SetLastError(NO_ERROR);
    }
    let low = unsafe { GetCompressedFileSizeW(path_w.as_ptr(), &mut high) };

    if low == INVALID_FILE_SIZE {
        let err = unsafe { GetLastError() };
        if err != NO_ERROR {
            let access_denied = err == ERROR_ACCESS_DENIED;
            let fallback = if err == ERROR_SHARING_VIOLATION || access_denied {
                logical_size
            } else if err == ERROR_FILE_NOT_FOUND || err == ERROR_PATH_NOT_FOUND {
                0
            } else {
                logical_size
            };
            return SizeOnDiskOutcome {
                size_on_disk: fallback,
                access_denied,
                error_code: Some(err),
            };
        }
    }

    let size_on_disk = ((high as u64) << 32) | (low as u64);
    SizeOnDiskOutcome {
        size_on_disk,
        access_denied: false,
        error_code: None,
    }
}

/// Fallback check if `dwReserved0` was 0 for an entry with `FILE_ATTRIBUTE_REPARSE_POINT`.
pub fn is_symlink_fallback(ext_path: &OsStr) -> bool {
    std::fs::symlink_metadata(ext_path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(true)
}

/// Enumerates fixed and removable Windows drives using Win32 APIs.
/// Handles `ERROR_NOT_READY` on empty removable slots gracefully.
pub fn list_drives() -> Vec<DriveInfo> {
    let mask = unsafe { GetLogicalDrives() };
    let mut drives = Vec::new();

    for idx in 0..26u8 {
        if (mask & (1u32 << idx)) == 0 {
            continue;
        }
        let letter_char = (b'A' + idx) as char;
        let letter = format!("{letter_char}:");
        let root_path = format!("{letter_char}:\\");
        let root_w = to_utf16_null(OsStr::new(&root_path));

        let drive_type_code = unsafe { GetDriveTypeW(root_w.as_ptr()) };
        let drive_type = match drive_type_code {
            DRIVE_FIXED => "fixed",
            DRIVE_REMOVABLE => "removable",
            _ => continue,
        };

        let mut free_avail: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut free_bytes: u64 = 0;
        let ok_space = unsafe {
            GetDiskFreeSpaceExW(
                root_w.as_ptr(),
                &mut free_avail,
                &mut total_bytes,
                &mut free_bytes,
            )
        };

        let is_ready = if ok_space != 0 {
            true
        } else {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_READY {
                total_bytes = 0;
                free_bytes = 0;
            }
            false
        };

        let mut label_buf = [0u16; 260];
        let mut fs_buf = [0u16; 64];
        let ok_vol = if is_ready {
            unsafe {
                GetVolumeInformationW(
                    root_w.as_ptr(),
                    label_buf.as_mut_ptr(),
                    label_buf.len() as u32,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    fs_buf.as_mut_ptr(),
                    fs_buf.len() as u32,
                )
            }
        } else {
            0
        };

        let label = if ok_vol != 0 {
            utf16_slice_to_string(&label_buf)
        } else {
            String::new()
        };
        let file_system = if ok_vol != 0 {
            utf16_slice_to_string(&fs_buf)
        } else {
            String::new()
        };

        let used_bytes = total_bytes.saturating_sub(free_bytes);
        let root_as_path = Path::new(&root_path);
        let is_protected =
            safety::is_critical_system_path(root_as_path) || safety::is_drive_root(root_as_path);
        let is_deletable = safety::is_deletable_path(root_as_path) && !is_protected;

        drives.push(DriveInfo {
            letter,
            root_path,
            label,
            file_system,
            drive_type: drive_type.to_string(),
            total_bytes,
            free_bytes,
            used_bytes,
            is_ready,
            is_protected,
            is_deletable,
        });
    }

    drives
}

/// Resolves standard Windows user folders (`Descargas`, `Escritorio`, `Imágenes`, `Documentos`, `Videos`)
/// via `SHGetKnownFolderPath` + `CoTaskMemFree`.
pub fn list_user_folders() -> Vec<UserFolderInfo> {
    let specs = [
        (UserFolderKind::Downloads, "Descargas", &FOLDERID_Downloads),
        (UserFolderKind::Desktop, "Escritorio", &FOLDERID_Desktop),
        (UserFolderKind::Pictures, "Imágenes", &FOLDERID_Pictures),
        (UserFolderKind::Documents, "Documentos", &FOLDERID_Documents),
        (UserFolderKind::Videos, "Videos", &FOLDERID_Videos),
    ];

    let mut folders = Vec::with_capacity(specs.len());
    for (kind, display_name, guid) in specs {
        if let Some(resolved_path) = resolve_known_folder(guid) {
            let clean = strip_extended_prefix_str(&resolved_path);
            let p = Path::new(&clean);
            let exists = p.exists();
            let is_protected = safety::is_critical_system_path(p);
            let is_deletable = safety::is_deletable_path(p);
            folders.push(UserFolderInfo {
                kind,
                name: display_name.to_string(),
                path: clean,
                exists,
                is_protected,
                is_deletable,
            });
        }
    }
    folders
}

fn resolve_known_folder(guid: &windows_sys::core::GUID) -> Option<String> {
    let mut ptr: *mut u16 = null_mut();
    let hr = unsafe { SHGetKnownFolderPath(guid, 0, null_mut(), &mut ptr) };
    let result = if hr == 0 && !ptr.is_null() {
        let mut len = 0usize;
        unsafe {
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            Some(String::from_utf16_lossy(slice))
        }
    } else {
        None
    };

    if !ptr.is_null() {
        unsafe {
            CoTaskMemFree(ptr.cast());
        }
    }
    result
}

fn utf16_slice_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows_sys::Win32::System::SystemServices::{
        IO_REPARSE_TAG_CLOUD_1, IO_REPARSE_TAG_CLOUD_F,
    };

    #[test]
    fn cloud_only_attribute_classification_avoids_unpinned_false_positive() {
        assert!(is_cloud_only_attributes(FILE_ATTRIBUTE_OFFLINE));
        assert!(is_cloud_only_attributes(
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
        ));
        assert!(is_cloud_only_attributes(FILE_ATTRIBUTE_RECALL_ON_OPEN));
        assert!(is_cloud_only_attributes(
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_UNPINNED
        ));

        assert!(!is_cloud_only_attributes(FILE_ATTRIBUTE_UNPINNED));
        assert!(!is_cloud_only_attributes(FILE_ATTRIBUTE_PINNED));
        assert!(!is_cloud_only_attributes(FILE_ATTRIBUTE_NORMAL));
        assert!(!is_cloud_only_attributes(0));
    }

    #[test]
    fn reparse_tag_differentiates_cloud_folders_from_symlinks_and_junctions() {
        assert!(is_cloud_reparse_tag(IO_REPARSE_TAG_CLOUD));
        assert!(is_cloud_reparse_tag(IO_REPARSE_TAG_CLOUD_1));
        assert!(is_cloud_reparse_tag(IO_REPARSE_TAG_CLOUD_F));
        assert!(!is_cloud_reparse_tag(IO_REPARSE_TAG_MOUNT_POINT));
        assert!(!is_cloud_reparse_tag(IO_REPARSE_TAG_SYMLINK));
        assert!(!is_cloud_reparse_tag(0));

        assert!(!is_name_surrogate_reparse_point(
            FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT,
            IO_REPARSE_TAG_CLOUD
        ));
        assert!(!is_name_surrogate_reparse_point(
            FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT,
            IO_REPARSE_TAG_CLOUD_1
        ));

        assert!(is_name_surrogate_reparse_point(
            FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT,
            IO_REPARSE_TAG_MOUNT_POINT
        ));
        assert!(is_name_surrogate_reparse_point(
            FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT,
            IO_REPARSE_TAG_SYMLINK
        ));
        assert!(!is_name_surrogate_reparse_point(
            FILE_ATTRIBUTE_DIRECTORY,
            IO_REPARSE_TAG_MOUNT_POINT
        ));
    }

    #[test]
    fn lists_drives_and_user_folders_without_crashing() {
        let drives = list_drives();
        assert!(
            !drives.is_empty(),
            "at least one Windows drive should be enumerated"
        );
        assert!(drives
            .iter()
            .any(|d| d.root_path.eq_ignore_ascii_case(r"C:\")));

        let folders = list_user_folders();
        assert!(!folders.is_empty());
        for folder in &folders {
            assert!(!folder.path.is_empty());
            assert!(!folder.is_protected);
            assert!(folder.is_deletable);
        }
    }
}
