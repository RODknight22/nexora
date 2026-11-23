use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Converts a path into a Windows extended-length path (`\\?\...` or `\\?\UNC\...`)
/// so Win32 APIs work reliably beyond MAX_PATH (260 UTF-16 code units).
pub fn to_extended_length_path(path: &Path) -> OsString {
    let raw = path.to_string_lossy();
    let replaced = raw.replace('/', "\\");

    if let Some(rest) = replaced.strip_prefix("\\\\?\\") {
        let clean = if let Some(unc_tail) = strip_unc_prefix(rest) {
            lexical_clean_windows(&format!("\\\\{unc_tail}"), false)
        } else {
            lexical_clean_windows(rest, false)
        };
        return format_extended_from_clean(&clean);
    }

    if let Some(rest) = replaced
        .strip_prefix("\\\\.\\")
        .or_else(|| replaced.strip_prefix("\\??\\"))
    {
        if let Some(unc_tail) = strip_unc_prefix(rest) {
            let clean = lexical_clean_windows(&format!("\\\\{unc_tail}"), true);
            return format_extended_from_clean(&clean);
        }
        if starts_with_drive_prefix(rest) {
            let clean = lexical_clean_windows(rest, true);
            return format_extended_from_clean(&clean);
        }
        return OsString::from(replaced);
    }

    let abs_path: PathBuf = if path.is_absolute() {
        PathBuf::from(replaced)
    } else {
        std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(replaced))
    };

    let clean = lexical_clean_windows(&abs_path.to_string_lossy().replace('/', "\\"), true);
    format_extended_from_clean(&clean)
}

fn strip_unc_prefix(s: &str) -> Option<&str> {
    if s.as_bytes()
        .get(..4)
        .is_some_and(|head| head.eq_ignore_ascii_case(b"UNC\\"))
    {
        Some(&s[4..])
    } else {
        None
    }
}

fn starts_with_drive_prefix(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn format_extended_from_clean(clean: &str) -> OsString {
    if let Some(unc_tail) = clean.strip_prefix("\\\\") {
        OsString::from(format!("\\\\?\\UNC\\{unc_tail}"))
    } else {
        OsString::from(format!("\\\\?\\{clean}"))
    }
}

fn is_preserved_extended_namespace(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    let starts_ci = |prefix: &[u8]| {
        bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix)
    };
    starts_ci(b"Volume{")
        || starts_ci(b"GLOBALROOT\\")
        || rest.eq_ignore_ascii_case("GLOBALROOT")
        || starts_ci(b"BootPartition\\")
        || rest.eq_ignore_ascii_case("BootPartition")
}

/// Strips `\\?\` or converts `\\?\UNC\` back to `\\` for clean display paths,
/// while preserving `\\?\` for Volume GUID and NT device namespaces (`Volume{...}`,
/// `GLOBALROOT`, `BootPartition`).
pub fn strip_extended_prefix(path: &Path) -> String {
    strip_extended_prefix_str(&path.to_string_lossy())
}

pub fn strip_extended_prefix_str(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("\\\\?\\") {
        if let Some(unc_tail) = strip_unc_prefix(rest) {
            return format!("\\\\{unc_tail}");
        }
        if is_preserved_extended_namespace(rest) {
            return raw.to_string();
        }
        return rest.to_string();
    }
    raw.to_string()
}

/// Encodes an `OsStr` as a null-terminated UTF-16 buffer for Win32 wide APIs.
pub fn to_utf16_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

/// Converts a path to its extended-length form and encodes it as null-terminated UTF-16.
pub fn to_extended_utf16(path: &Path) -> Vec<u16> {
    let ext = to_extended_length_path(path);
    to_utf16_null(&ext)
}

/// Extracts a lowercase file extension without the leading dot.
pub fn extract_extension(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
        .map(|ext| ext.to_ascii_lowercase())
}

/// Joins a child segment onto a display path cleanly without introducing duplicate separators.
pub fn join_display_path(parent: &str, child_name: &str) -> String {
    if parent.ends_with('\\') || parent.ends_with('/') {
        format!("{parent}{child_name}")
    } else {
        format!("{parent}\\{child_name}")
    }
}

/// Joins a child segment onto an extended-length path cleanly.
pub fn join_extended_path(parent: &OsStr, child_name: &str) -> OsString {
    let parent_str = parent.to_string_lossy();
    if parent_str.ends_with('\\') {
        OsString::from(format!("{parent_str}{child_name}"))
    } else {
        OsString::from(format!("{parent_str}\\{child_name}"))
    }
}

fn strip_ntfs_ads(segment: &str) -> &str {
    segment.split_once(':').map_or(segment, |(head, _)| head)
}

fn next_non_empty_unc_segment(input: &str) -> (&str, &str) {
    let trimmed = input.trim_start_matches('\\');
    match trimmed.find('\\') {
        Some(idx) => (&trimmed[..idx], trimmed[idx..].trim_start_matches('\\')),
        None => (trimmed, ""),
    }
}

fn clean_segment(segment: &str, trim_win32_trailing: bool) -> &str {
    let without_ads = strip_ntfs_ads(segment);
    if trim_win32_trailing {
        without_ads.trim_end_matches([' ', '.'])
    } else {
        without_ads
    }
}

fn lexical_clean_windows(raw: &str, trim_win32_trailing: bool) -> String {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = &raw[..2];
        let rest = raw[2..].trim_start_matches('\\');
        let mut stack: Vec<&str> = Vec::new();
        for seg in rest.split('\\') {
            if seg.is_empty() || seg == "." {
                continue;
            }
            if seg == ".." {
                stack.pop();
                continue;
            }
            let cleaned = clean_segment(seg, trim_win32_trailing);
            if cleaned.is_empty() {
                continue;
            }
            stack.push(cleaned);
        }
        if stack.is_empty() {
            return format!("{drive}\\");
        }
        return format!("{drive}\\{}", stack.join("\\"));
    }

    if let Some(unc) = raw.strip_prefix("\\\\") {
        let (raw_server, after_server) = next_non_empty_unc_segment(unc);
        let (raw_share, rest) = next_non_empty_unc_segment(after_server);
        let server = if trim_win32_trailing {
            raw_server.trim_end_matches([' ', '.'])
        } else {
            raw_server
        };
        let share = clean_segment(raw_share, trim_win32_trailing);
        let mut stack: Vec<&str> = Vec::new();
        for seg in rest.split('\\') {
            if seg.is_empty() || seg == "." {
                continue;
            }
            if seg == ".." {
                stack.pop();
                continue;
            }
            let cleaned = clean_segment(seg, trim_win32_trailing);
            if cleaned.is_empty() {
                continue;
            }
            stack.push(cleaned);
        }
        if server.is_empty() {
            return String::from("\\\\");
        }
        if share.is_empty() {
            return format!("\\\\{server}");
        }
        if stack.is_empty() {
            return format!("\\\\{server}\\{share}");
        }
        return format!("\\\\{server}\\{share}\\{}", stack.join("\\"));
    }

    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_and_strips_extended_paths() {
        let p = Path::new(r"C:\Users\Alice\Documents\file.txt");
        let ext = to_extended_length_path(p);
        assert_eq!(
            ext.to_string_lossy(),
            r"\\?\C:\Users\Alice\Documents\file.txt"
        );
        assert_eq!(
            strip_extended_prefix(Path::new(&ext)),
            r"C:\Users\Alice\Documents\file.txt"
        );

        let unc = Path::new(r"\\Server\Share\Folder\item.bin");
        let ext_unc = to_extended_length_path(unc);
        assert_eq!(
            ext_unc.to_string_lossy(),
            r"\\?\UNC\Server\Share\Folder\item.bin"
        );
        assert_eq!(
            strip_extended_prefix(Path::new(&ext_unc)),
            r"\\Server\Share\Folder\item.bin"
        );
    }

    #[test]
    fn normalizes_device_nt_extended_and_trailing_win32_paths() {
        assert_eq!(
            to_extended_length_path(Path::new(r"\\.\C:\Windows")).to_string_lossy(),
            r"\\?\C:\Windows"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\??\C:\Windows")).to_string_lossy(),
            r"\\?\C:\Windows"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\\.\UNC\Server\Share\Sub")).to_string_lossy(),
            r"\\?\UNC\Server\Share\Sub"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\\?\C:\Users\..\Windows")).to_string_lossy(),
            r"\\?\C:\Windows"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\\?\UNC\Server\Share\a\..\b")).to_string_lossy(),
            r"\\?\UNC\Server\Share\b"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"C:\Windows ")).to_string_lossy(),
            r"\\?\C:\Windows"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"C:\Windows.")).to_string_lossy(),
            r"\\?\C:\Windows"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"C:\Windows.old")).to_string_lossy(),
            r"\\?\C:\Windows.old"
        );
    }

    #[test]
    fn handles_non_ascii_prefixed_paths_without_panicking() {
        assert_eq!(
            to_extended_length_path(Path::new(r"\\?\C:\Árbol")).to_string_lossy(),
            r"\\?\C:\Árbol"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\\.\C:\Óscar")).to_string_lossy(),
            r"\\?\C:\Óscar"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\??\D:\日本語")).to_string_lossy(),
            r"\\?\D:\日本語"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\\?\C:\Árbol\..")).to_string_lossy(),
            r"\\?\C:\"
        );
    }

    #[test]
    fn strips_ntfs_ads_and_collapses_unc_separators() {
        assert_eq!(
            to_extended_length_path(Path::new(r"C:\::$INDEX_ALLOCATION")).to_string_lossy(),
            r"\\?\C:\"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\\?\C:\::$INDEX_ALLOCATION")).to_string_lossy(),
            r"\\?\C:\"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"D:\::$INDEX_ALLOCATION")).to_string_lossy(),
            r"\\?\D:\"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"C:\Windows::$INDEX_ALLOCATION")).to_string_lossy(),
            r"\\?\C:\Windows"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\\.\C:\Windows::$INDEX_ALLOCATION"))
                .to_string_lossy(),
            r"\\?\C:\Windows"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"C:\Program Files::$INDEX_ALLOCATION"))
                .to_string_lossy(),
            r"\\?\C:\Program Files"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"C:\PROGRA~1::$INDEX_ALLOCATION")).to_string_lossy(),
            r"\\?\C:\PROGRA~1"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\\localhost\\C$\Windows")).to_string_lossy(),
            r"\\?\UNC\localhost\C$\Windows"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\\?\UNC\localhost\\C$\Windows")).to_string_lossy(),
            r"\\?\UNC\localhost\C$\Windows"
        );
        assert_eq!(
            to_extended_length_path(Path::new(r"\\localhost\C$::$INDEX_ALLOCATION\Windows"))
                .to_string_lossy(),
            r"\\?\UNC\localhost\C$\Windows"
        );
    }

    #[test]
    fn extracts_lowercase_extensions() {
        assert_eq!(extract_extension("setup.EXE"), Some(String::from("exe")));
        assert_eq!(
            extract_extension("archive.tar.gz"),
            Some(String::from("gz"))
        );
        assert_eq!(extract_extension("README"), None);
        assert_eq!(extract_extension(".gitignore"), None);
    }

    #[test]
    fn strips_mixed_case_unc_prefix_and_preserves_volume_and_device_namespaces() {
        assert_eq!(
            strip_extended_prefix_str(r"\\?\Unc\Server\Share\File"),
            r"\\Server\Share\File"
        );
        assert_eq!(
            strip_extended_prefix_str(r"\\?\uNc\Server\Share\File"),
            r"\\Server\Share\File"
        );

        let preserved = [
            r"\\?\Volume{36d7375e-dea6-4086-a7ce-9e54afa0ccfe}\Windows",
            r"\\?\Volume{36d7375e-dea6-4086-a7ce-9e54afa0ccfe}\",
            r"\\?\GLOBALROOT\Device\HarddiskVolume3\Windows",
            r"\\?\BootPartition\Windows",
        ];
        for path_str in preserved {
            assert_eq!(strip_extended_prefix_str(path_str), path_str);
            assert_eq!(
                to_extended_length_path(Path::new(path_str)).to_string_lossy(),
                path_str
            );
        }

        for device_str in [
            r"\\.\BootPartition\Windows",
            r"\\.\GLOBALROOT\Device\HarddiskVolume3\Windows",
            r"\??\GLOBALROOT\Device\HarddiskVolume3\Windows",
        ] {
            assert_eq!(
                to_extended_length_path(Path::new(device_str)).to_string_lossy(),
                device_str
            );
        }
    }
}
