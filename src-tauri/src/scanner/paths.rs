use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Converts a path into a Windows extended-length path (`\\?\...` or `\\?\UNC\...`)
/// so Win32 APIs work reliably beyond MAX_PATH (260 UTF-16 code units).
pub fn to_extended_length_path(path: &Path) -> OsString {
    let raw = path.to_string_lossy();
    let replaced = raw.replace('/', "\\");

    if replaced.starts_with("\\\\?\\") {
        return OsString::from(replaced);
    }

    let abs_path: PathBuf = if path.is_absolute() {
        PathBuf::from(replaced)
    } else {
        std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(replaced))
    };

    let clean = lexical_clean_windows(&abs_path.to_string_lossy().replace('/', "\\"));

    if let Some(unc_tail) = clean.strip_prefix("\\\\") {
        OsString::from(format!("\\\\?\\UNC\\{unc_tail}"))
    } else {
        OsString::from(format!("\\\\?\\{clean}"))
    }
}

/// Strips `\\?\` or converts `\\?\UNC\` back to `\\` for clean display paths.
pub fn strip_extended_prefix(path: &Path) -> String {
    strip_extended_prefix_str(&path.to_string_lossy())
}

pub fn strip_extended_prefix_str(raw: &str) -> String {
    if let Some(unc_tail) = raw
        .strip_prefix("\\\\?\\UNC\\")
        .or_else(|| raw.strip_prefix("\\\\?\\unc\\"))
    {
        format!("\\\\{unc_tail}")
    } else if let Some(rest) = raw.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        raw.to_string()
    }
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

fn lexical_clean_windows(raw: &str) -> String {
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
            stack.push(seg);
        }
        if stack.is_empty() {
            return format!("{drive}\\");
        }
        return format!("{drive}\\{}", stack.join("\\"));
    }

    if let Some(unc) = raw.strip_prefix("\\\\") {
        let mut parts = unc.trim_start_matches('\\').splitn(3, '\\');
        let server = parts.next().unwrap_or("");
        let share = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        let mut stack: Vec<&str> = Vec::new();
        for seg in rest.split('\\') {
            if seg.is_empty() || seg == "." {
                continue;
            }
            if seg == ".." {
                stack.pop();
                continue;
            }
            stack.push(seg);
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
    fn extracts_lowercase_extensions() {
        assert_eq!(extract_extension("setup.EXE"), Some(String::from("exe")));
        assert_eq!(
            extract_extension("archive.tar.gz"),
            Some(String::from("gz"))
        );
        assert_eq!(extract_extension("README"), None);
        assert_eq!(extract_extension(".gitignore"), None);
    }
}
