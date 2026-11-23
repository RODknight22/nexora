use std::path::Path;

/// Normalizes a Windows path for consistent comparisons and protection checks:
/// - Strips `\\?\` and converts `\\?\UNC\` to `\\`
/// - Converts `/` to `\`
/// - Resolves `.` and `..` components without escaping drive/UNC roots
/// - Trims trailing `\` except for drive roots (e.g. `c:\`)
/// - Lowercases ASCII/Unicode characters
pub fn normalize_windows_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    normalize_windows_path_str(&raw)
}

pub fn normalize_windows_path_str(raw: &str) -> String {
    let replaced = raw.replace('/', "\\");
    let stripped = if let Some(unc_rest) = replaced
        .strip_prefix("\\\\?\\UNC\\")
        .or_else(|| replaced.strip_prefix("\\\\?\\unc\\"))
    {
        format!("\\\\{unc_rest}")
    } else if let Some(rest) = replaced.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        replaced
    };

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
        stack.push(segment.to_lowercase());
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
        let server = parts.next().unwrap_or("");
        let share = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        if !server.is_empty() && !share.is_empty() {
            return (
                format!("\\\\{}\\{}", server.to_lowercase(), share.to_lowercase()),
                rest,
                true,
            );
        }
        return ("\\\\".to_string(), trimmed, true);
    }

    if let Some(rest) = s.strip_prefix('\\') {
        return ("\\".to_string(), rest, true);
    }

    (String::new(), s, false)
}

/// Returns true if the path represents a Windows drive root such as `C:\` or `D:\`.
pub fn is_drive_root(path: &Path) -> bool {
    let norm = normalize_windows_path(path);
    let bytes = norm.as_bytes();
    bytes.len() == 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

/// Returns true if `path` is a critical system directory (or inside one) or the system/drive root (`C:\`).
/// Uses component-boundary matching so `C:\WindowsBackup` is NOT matched as `C:\Windows`.
pub fn is_critical_system_path(path: &Path) -> bool {
    let norm = normalize_windows_path(path);
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

    if exact_roots.contains(&norm) {
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

/// Returns true if the path is safe to consider for deletion (i.e. not a critical system path).
pub fn is_deletable_path(path: &Path) -> bool {
    !is_critical_system_path(path)
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
}
