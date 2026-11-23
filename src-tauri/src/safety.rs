use std::path::Path;

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;

/// Normalizes a Windows path for consistent comparisons and protection checks:
/// - Strips `\\?\`, `\\.\`, `\??\` and converts `UNC\` forms to `\\`
/// - Converts `/` to `\`
/// - Maps loopback administrative shares (`\\localhost\<drive>$`, `\\127.0.0.1\<drive>$`) to `<drive>:\`
/// - Resolves `.` and `..` components without escaping drive/UNC roots
/// - Strips Win32 trailing spaces and dots from non-root segments (preserving `Windows.old`)
/// - Resolves NTFS 8.3 short names (`~`) for existing paths via `GetLongPathNameW`
/// - Trims trailing `\` except for drive roots (e.g. `c:\`)
/// - Lowercases ASCII/Unicode characters
pub fn normalize_windows_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    normalize_windows_path_str(&raw)
}

pub fn normalize_windows_path_str(raw: &str) -> String {
    let lexical = lexical_normalize_windows_str(raw);
    if lexical.is_empty() {
        return lexical;
    }

    #[cfg(windows)]
    if lexical.contains('~') && is_drive_or_unc_path(&lexical) {
        if let Some(resolved) = resolve_long_path_windows(&lexical) {
            return lexical_normalize_windows_str(&resolved);
        }
    }

    lexical
}

fn lexical_normalize_windows_str(raw: &str) -> String {
    let replaced = raw.replace('/', "\\");
    let stripped = strip_namespace_prefix(&replaced);

    let (prefix, remainder, rooted) = split_windows_prefix(&stripped);
    let mut stack: Vec<String> = Vec::new();

    for segment in remainder.split('\\') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            if let Some(last) = stack.last() {
                if last != ".." {
                    stack.pop();
                } else if !rooted {
                    stack.push("..".to_string());
                }
            } else if !rooted {
                stack.push("..".to_string());
            }
            continue;
        }
        let cleaned = segment.trim_end_matches([' ', '.']);
        if cleaned.is_empty() {
            continue;
        }
        stack.push(cleaned.to_lowercase());
    }

    if prefix.is_empty() {
        stack.join("\\")
    } else if prefix.ends_with('\\') {
        format!("{prefix}{}", stack.join("\\"))
    } else if stack.is_empty() {
        prefix
    } else {
        format!("{prefix}\\{}", stack.join("\\"))
    }
}

fn strip_namespace_prefix(s: &str) -> String {
    for prefix in ["\\\\?\\", "\\\\.\\", "\\??\\"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case("UNC\\") {
                return format!("\\\\{}", &rest[4..]);
            }
            return rest.to_string();
        }
    }
    s.to_string()
}

fn split_windows_prefix(s: &str) -> (String, &str, bool) {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let after_colon = &s[2..];
        let remainder = after_colon.trim_start_matches('\\');
        return (format!("{drive}:\\"), remainder, true);
    }

    if let Some(unc_body) = s.strip_prefix("\\\\") {
        let trimmed = unc_body.trim_start_matches('\\');
        let mut parts = trimmed.splitn(3, '\\');
        let server = parts.next().unwrap_or("").trim_end_matches([' ', '.']);
        let share = parts.next().unwrap_or("").trim_end_matches([' ', '.']);
        let rest = parts.next().unwrap_or("");
        if !server.is_empty() && !share.is_empty() {
            let server_lower = server.to_lowercase();
            let share_lower = share.to_lowercase();
            let share_bytes = share_lower.as_bytes();
            let is_loopback = matches!(server_lower.as_str(), "localhost" | "127.0.0.1" | "::1");
            if is_loopback
                && share_bytes.len() == 2
                && share_bytes[0].is_ascii_alphabetic()
                && share_bytes[1] == b'$'
            {
                let drive = share_bytes[0] as char;
                return (format!("{drive}:\\"), rest.trim_start_matches('\\'), true);
            }
            return (format!("\\\\{server_lower}\\{share_lower}"), rest, true);
        }
        return ("\\\\".to_string(), trimmed, true);
    }

    if let Some(rest) = s.strip_prefix('\\') {
        return ("\\".to_string(), rest, true);
    }

    (String::new(), s, false)
}

#[cfg(windows)]
fn is_drive_or_unc_path(norm: &str) -> bool {
    let bytes = norm.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
        || norm.starts_with("\\\\")
}

#[cfg(windows)]
fn to_extended_win32_str(norm: &str) -> String {
    if let Some(unc_tail) = norm.strip_prefix("\\\\") {
        format!("\\\\?\\UNC\\{unc_tail}")
    } else {
        format!("\\\\?\\{norm}")
    }
}

#[cfg(windows)]
fn call_get_long_path_name(norm_candidate: &str) -> Option<String> {
    let ext = to_extended_win32_str(norm_candidate);
    let wide: Vec<u16> = OsString::from(ext)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut buf = vec![0u16; wide.len().max(260)];
    let mut len = unsafe { GetLongPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 {
        return None;
    }
    if (len as usize) >= buf.len() {
        buf.resize((len as usize) + 1, 0);
        len = unsafe { GetLongPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if len == 0 || (len as usize) >= buf.len() {
            return None;
        }
    }

    let resolved = OsString::from_wide(&buf[..len as usize]);
    Some(resolved.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn resolve_long_path_windows(lexical: &str) -> Option<String> {
    if let Some(full) = call_get_long_path_name(lexical) {
        return Some(full);
    }

    // Walk up parent boundaries so existing short-name ancestors (e.g. `C:\PROGRA~1\NewDir`)
    // still resolve even when the leaf does not exist on disk.
    let mut split_pos = lexical.len();
    while let Some(slash_idx) = lexical[..split_pos].rfind('\\') {
        let prefix = &lexical[..slash_idx];
        if prefix.len() <= 3 || !prefix.contains('~') {
            break;
        }
        if let Some(resolved_prefix) = call_get_long_path_name(prefix) {
            let tail = &lexical[slash_idx..];
            return Some(format!("{resolved_prefix}{tail}"));
        }
        split_pos = slash_idx;
    }

    None
}

fn is_drive_root_norm(norm: &str) -> bool {
    let bytes = norm.as_bytes();
    bytes.len() == 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

/// Returns true if the path represents a Windows drive root such as `C:\` or `D:\`.
pub fn is_drive_root(path: &Path) -> bool {
    let norm = normalize_windows_path(path);
    is_drive_root_norm(&norm)
}

fn is_critical_system_path_norm(norm: &str) -> bool {
    if norm.is_empty() {
        return false;
    }

    let mut exact_roots = vec![String::from("c:\\")];
    if let Ok(sys_drive) = std::env::var("SystemDrive") {
        let trimmed = sys_drive.trim();
        if !trimmed.is_empty() {
            let root_candidate = format!("{}\\", trimmed.trim_end_matches(['\\', '/']));
            let root_norm = normalize_windows_path_str(&root_candidate);
            if !root_norm.is_empty() && !exact_roots.contains(&root_norm) {
                exact_roots.push(root_norm);
            }
        }
    }

    if exact_roots.contains(&norm.to_string()) {
        return true;
    }

    let mut protected_trees = vec![
        String::from("c:\\windows"),
        String::from("c:\\program files"),
        String::from("c:\\program files (x86)"),
        String::from("c:\\programdata"),
    ];

    for env_key in [
        "SystemRoot",
        "WINDIR",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
    ] {
        if let Ok(val) = std::env::var(env_key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                let candidate = normalize_windows_path_str(trimmed);
                if candidate.len() > 3 && !protected_trees.contains(&candidate) {
                    protected_trees.push(candidate);
                }
            }
        }
    }

    protected_trees.iter().any(|tree| {
        if norm == *tree {
            return true;
        }
        let prefix = format!("{}\\", tree.trim_end_matches('\\'));
        norm.starts_with(&prefix)
    })
}

/// Returns true if `path` is a critical system directory (or inside one) or the system/drive root (`C:\`).
/// Uses component-boundary matching so `C:\WindowsBackup` is NOT matched as `C:\Windows`.
pub fn is_critical_system_path(path: &Path) -> bool {
    let norm = normalize_windows_path(path);
    is_critical_system_path_norm(&norm)
}

/// Returns true if the path is safe to consider for deletion (i.e. non-empty, not a drive root,
/// and not a critical system path).
pub fn is_deletable_path(path: &Path) -> bool {
    let norm = normalize_windows_path(path);
    if norm.is_empty() || is_drive_root_norm(&norm) {
        return false;
    }
    !is_critical_system_path_norm(&norm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_slashes_extended_prefixes_case_and_dots() {
        assert_eq!(
            normalize_windows_path(Path::new(r"\\?\C:\Windows\System32\..\")),
            r"c:\windows"
        );
        assert_eq!(
            normalize_windows_path(Path::new("c:/Program Files (x86)/App/")),
            r"c:\program files (x86)\app"
        );
        assert_eq!(normalize_windows_path(Path::new(r"\\?\C:\")), r"c:\");
        assert_eq!(
            normalize_windows_path(Path::new(r"\\?\UNC\NasServer\Share\Folder\")),
            r"\\nasserver\share\folder"
        );
    }

    #[test]
    fn protects_critical_system_paths_and_roots_without_false_positives() {
        assert!(is_critical_system_path(Path::new(r"C:\")));
        assert!(is_critical_system_path(Path::new(r"\\?\C:\")));
        assert!(is_critical_system_path(Path::new(r"C:\Windows")));
        assert!(is_critical_system_path(Path::new(r"\\?\C:\Windows")));
        assert!(is_critical_system_path(Path::new("c:/windows/")));
        assert!(is_critical_system_path(Path::new(
            r"C:\Windows\System32\drivers"
        )));
        assert!(is_critical_system_path(Path::new(r"C:\Program Files")));
        assert!(is_critical_system_path(Path::new(
            r"C:\Program Files\Common Files"
        )));
        assert!(is_critical_system_path(Path::new(
            r"C:\Program Files (x86)"
        )));
        assert!(is_critical_system_path(Path::new(r"C:\ProgramData")));
        assert!(is_critical_system_path(Path::new(
            r"C:\Users\..\Windows\Temp"
        )));

        assert!(!is_critical_system_path(Path::new(r"C:\WindowsBackup")));
        assert!(!is_critical_system_path(Path::new(
            r"C:\WindowsBackup\data.bin"
        )));
        assert!(!is_critical_system_path(Path::new(r"C:\ProgramDataExtra")));
        assert!(!is_critical_system_path(Path::new(
            r"C:\Users\rodkn\Downloads"
        )));

        assert!(!is_deletable_path(Path::new(r"C:\Windows")));
        assert!(!is_deletable_path(Path::new(r"C:\")));
        assert!(is_deletable_path(Path::new(r"C:\WindowsBackup")));
        assert!(is_deletable_path(Path::new(r"C:\Users\rodkn\Downloads")));
    }

    #[test]
    fn identifies_drive_roots() {
        assert!(is_drive_root(Path::new(r"C:\")));
        assert!(is_drive_root(Path::new(r"\\?\D:\")));
        assert!(!is_drive_root(Path::new(r"C:\Users")));
    }

    #[test]
    fn blocks_device_nt_trailing_dot_space_and_loopback_share_bypasses() {
        for critical_bypass in [
            r"\\.\C:\Windows",
            r"\??\C:\Windows",
            r"\\.\C:\",
            r"\??\C:\",
            r"\\.\UNC\localhost\C$\Windows",
            r"\??\UNC\127.0.0.1\C$\Windows",
            r"C:\Windows ",
            r"C:\Windows.",
            r"C:\Windows . .",
            r"C:\Users\.\..\Windows ",
            r"\\localhost\C$\Windows",
            r"\\127.0.0.1\C$\Windows",
            r"\\?\UNC\localhost\C$\Windows",
            r"\\?\UNC\127.0.0.1\C$\Windows",
            r"\\localhost\C$",
        ] {
            assert!(
                is_critical_system_path(Path::new(critical_bypass)),
                "expected critical system path for {critical_bypass}"
            );
            assert!(
                !is_deletable_path(Path::new(critical_bypass)),
                "expected non-deletable path for {critical_bypass}"
            );
        }

        assert_eq!(
            normalize_windows_path(Path::new(r"C:\Windows.old")),
            r"c:\windows.old"
        );
        assert!(!is_critical_system_path(Path::new(r"C:\Windows.old")));
        assert!(is_deletable_path(Path::new(r"C:\Windows.old")));
        assert!(is_deletable_path(Path::new(r"C:\WindowsBackup")));
        assert!(is_deletable_path(Path::new(r"D:\Data")));
        assert!(is_deletable_path(Path::new(r"\\localhost\D$\Data")));

        assert!(!is_deletable_path(Path::new(r"D:\")));
        assert!(!is_deletable_path(Path::new(r"D:")));
        assert!(!is_deletable_path(Path::new(r"\\localhost\D$\")));
        assert!(!is_deletable_path(Path::new("")));
        assert!(!is_deletable_path(Path::new("   ")));
    }

    #[test]
    fn resolves_existing_ntfs_8_3_short_names_and_falls_back_gracefully() {
        let short_cases = [
            (r"C:\PROGRA~1", r"c:\program files"),
            (r"C:\PROGRA~2", r"c:\program files (x86)"),
            (r"C:\PROGRA~3", r"c:\programdata"),
        ];

        for (short_path, expected_norm) in short_cases {
            if Path::new(short_path).exists() {
                assert_eq!(normalize_windows_path(Path::new(short_path)), expected_norm);
                assert!(is_critical_system_path(Path::new(short_path)));
                assert!(!is_deletable_path(Path::new(short_path)));

                let nested_nonexistent = format!(r"{short_path}\NonExistentSub\item.bin");
                assert!(is_critical_system_path(Path::new(&nested_nonexistent)));
                assert!(!is_deletable_path(Path::new(&nested_nonexistent)));
            }
        }

        // Non-existent short-name path falls back cleanly without crashing
        let fallback = normalize_windows_path(Path::new(r"C:\NONEXI~1\Sub"));
        assert_eq!(fallback, r"c:\nonexi~1\sub");
        assert!(!is_critical_system_path(Path::new(r"C:\NONEXI~1\Sub")));
    }
}
