use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, HANDLE,
    INVALID_HANDLE_VALUE,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetFinalPathNameByHandleW, GetLogicalDrives, GetLongPathNameW,
    GetVolumePathNamesForVolumeNameW, QueryDosDeviceW, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, VOLUME_NAME_DOS,
};
#[cfg(windows)]
use windows_sys::Win32::System::SystemInformation::{
    ComputerNameDnsFullyQualified, ComputerNameDnsHostname, ComputerNameNetBIOS,
    GetComputerNameExW, COMPUTER_NAME_FORMAT,
};

/// Normalizes a Windows path for consistent comparisons and protection checks:
/// - Resolves NT Device & Volume GUID namespaces (`\\?\Volume{...}`, `GLOBALROOT`, `BootPartition`, `\Device\...`) to DOS paths
/// - Resolves non-empty relative paths against the current working directory
/// - Strips `\\?\`, `\\.\`, `\??\` and converts `UNC\` forms to `\\`
/// - Converts `/` to `\`
/// - Maps local-host, loopback (`127.0.0.0/8`, `::1`, `*.ipv6-literal.net`), and local computer-name `<drive>$` shares to `<drive>:\`
/// - Resolves `.` and `..` components without escaping drive/UNC roots
/// - Strips NTFS ADS (`:stream`) and Win32 trailing spaces/dots from non-root segments (preserving `Windows.old`)
/// - Resolves NTFS 8.3 short names (`~`) for existing paths via `GetLongPathNameW`
/// - Trims trailing `\` except for drive roots (e.g. `c:\`)
/// - Lowercases ASCII/Unicode characters
pub fn normalize_windows_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    normalize_windows_path_str(&raw)
}

pub fn normalize_windows_path_str(raw: &str) -> String {
    normalize_windows_path_with_base(raw, None)
}

fn normalize_windows_path_with_base(raw: &str, base_dir: Option<&Path>) -> String {
    if raw.trim().is_empty() {
        return String::new();
    }

    let replaced = raw.replace('/', "\\");

    let candidate = if is_nt_or_volume_input(&replaced) {
        if let Some(unc_tail) = strip_unc_namespace_prefix(&replaced) {
            format!("\\\\{unc_tail}")
        } else if let Some(drive_tail) = strip_drive_namespace_prefix(&replaced) {
            drive_tail.to_string()
        } else {
            #[cfg(windows)]
            {
                if let Some(resolved) = resolve_nt_or_volume_path_windows(&replaced, 0) {
                    resolved
                } else {
                    return format_unresolved_nt_namespace(&replaced);
                }
            }
            #[cfg(not(windows))]
            {
                return format_unresolved_nt_namespace(&replaced);
            }
        }
    } else if replaced.starts_with("\\\\")
        || is_drive_only(&replaced)
        || starts_with_drive_root(&replaced)
    {
        replaced
    } else {
        let joined = match base_dir {
            Some(base) => base.join(&replaced),
            None => Path::new(&replaced).to_path_buf(),
        };
        match std::path::absolute(&joined) {
            Ok(abs) => abs.to_string_lossy().replace('/', "\\"),
            Err(_) => replaced,
        }
    };

    let lexical = lexical_normalize_windows_str(&candidate);
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

fn is_drive_only(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn starts_with_drive_root(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

fn starts_with_ci_segment(s: &str, prefix: &str) -> bool {
    let sb = s.as_bytes();
    let pb = prefix.as_bytes();
    sb.len() >= pb.len()
        && sb[..pb.len()].eq_ignore_ascii_case(pb)
        && (sb.len() == pb.len() || sb[pb.len()] == b'\\')
}

fn is_nt_or_volume_input(s: &str) -> bool {
    s.starts_with("\\\\?\\")
        || s.starts_with("\\\\.\\")
        || s.starts_with("\\??\\")
        || starts_with_ci_segment(s, "\\Device")
        || starts_with_ci_segment(s, "\\DosDevices")
        || starts_with_ci_segment(s, "\\GLOBAL??")
}

fn strip_unc_namespace_prefix(s: &str) -> Option<&str> {
    for prefix in ["\\\\?\\", "\\\\.\\", "\\??\\"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if rest
                .as_bytes()
                .get(..4)
                .is_some_and(|head| head.eq_ignore_ascii_case(b"UNC\\"))
            {
                return Some(&rest[4..]);
            }
        }
    }
    None
}

fn strip_drive_namespace_prefix(s: &str) -> Option<&str> {
    for prefix in ["\\\\?\\", "\\\\.\\", "\\??\\"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if is_drive_only(rest) || starts_with_drive_root(rest) {
                return Some(rest);
            }
        }
    }
    None
}

fn format_unresolved_nt_namespace(s: &str) -> String {
    let stripped = strip_namespace_prefix(s);
    let mut parts: Vec<String> = Vec::new();
    for seg in stripped.split('\\') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        let cleaned = strip_ntfs_ads(seg).trim_end_matches([' ', '.']);
        if !cleaned.is_empty() {
            parts.push(cleaned.to_lowercase());
        }
    }
    if stripped.starts_with('\\') && !stripped.starts_with("\\\\") {
        format!("\\{}", parts.join("\\"))
    } else {
        parts.join("\\")
    }
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
        let cleaned = strip_ntfs_ads(segment).trim_end_matches([' ', '.']);
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

fn strip_namespace_prefix(s: &str) -> String {
    for prefix in ["\\\\?\\", "\\\\.\\", "\\??\\"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if rest
                .as_bytes()
                .get(..4)
                .is_some_and(|head| head.eq_ignore_ascii_case(b"UNC\\"))
            {
                return format!("\\\\{}", &rest[4..]);
            }
            return rest.to_string();
        }
    }
    s.to_string()
}

fn is_ipv6_loopback_host(host: &str) -> bool {
    let inner = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    let no_zone = inner.split('%').next().unwrap_or(inner);
    let Ok(ip6) = no_zone.parse::<Ipv6Addr>() else {
        return false;
    };
    ip6.is_loopback() || ip6.to_ipv4_mapped().is_some_and(|ip4| ip4.is_loopback())
}

fn is_ipv6_literal_net_loopback(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    let Some(prefix) = lower.strip_suffix(".ipv6-literal.net") else {
        return false;
    };
    // Strip optional zone suffix (`.s<zone>` or `s<zone>`; `s` never appears in IPv6 hex)
    let no_dot_zone = prefix.split(".s").next().unwrap_or(prefix);
    let base = no_dot_zone.split('s').next().unwrap_or(no_dot_zone);

    for candidate in [base.replace('-', ":"), base.replace("--", ":")] {
        if let Ok(ip6) = candidate.parse::<Ipv6Addr>() {
            if ip6.is_loopback() || ip6.to_ipv4_mapped().is_some_and(|ip4| ip4.is_loopback()) {
                return true;
            }
        }
    }
    false
}

#[cfg(windows)]
fn query_computer_name(format: COMPUTER_NAME_FORMAT) -> Option<String> {
    let mut size: u32 = 0;
    unsafe {
        GetComputerNameExW(format, std::ptr::null_mut(), &mut size);
    }
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u16; size as usize];
    let ok = unsafe { GetComputerNameExW(format, buf.as_mut_ptr(), &mut size) };
    if ok == 0 {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..size as usize]);
    let trimmed = s.trim_end_matches(['\0', ' ', '.']);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(windows)]
fn is_local_computer_name(host: &str) -> bool {
    if let Ok(env_name) = std::env::var("COMPUTERNAME") {
        let clean = env_name.trim().trim_end_matches('.');
        if !clean.is_empty() && host.eq_ignore_ascii_case(clean) {
            return true;
        }
    }
    for format in [
        ComputerNameNetBIOS,
        ComputerNameDnsHostname,
        ComputerNameDnsFullyQualified,
    ] {
        if let Some(name) = query_computer_name(format) {
            if host.eq_ignore_ascii_case(&name) {
                return true;
            }
        }
    }
    false
}

fn is_local_host_alias(server: &str) -> bool {
    let host = server.trim_end_matches([' ', '.']);
    if host.is_empty() {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(ip4) = host.parse::<Ipv4Addr>() {
        return ip4.is_loopback();
    }
    if is_ipv6_loopback_host(host) || is_ipv6_literal_net_loopback(host) {
        return true;
    }
    #[cfg(windows)]
    if is_local_computer_name(host) {
        return true;
    }
    false
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
        let (raw_server, after_server) = next_non_empty_unc_segment(unc_body);
        let (raw_share, rest) = next_non_empty_unc_segment(after_server);
        let server = raw_server.trim_end_matches([' ', '.']);
        let share = strip_ntfs_ads(raw_share).trim_end_matches([' ', '.']);
        if !server.is_empty() && !share.is_empty() {
            let server_lower = server.to_lowercase();
            let share_lower = share.to_lowercase();
            let share_bytes = share_lower.as_bytes();
            if is_local_host_alias(server)
                && share_bytes.len() == 2
                && share_bytes[0].is_ascii_alphabetic()
                && share_bytes[1] == b'$'
            {
                let drive = share_bytes[0] as char;
                return (format!("{drive}:\\"), rest, true);
            }
            return (format!("\\\\{server_lower}\\{share_lower}"), rest, true);
        }
        return ("\\\\".to_string(), unc_body.trim_start_matches('\\'), true);
    }

    if let Some(rest) = s.strip_prefix('\\') {
        return ("\\".to_string(), rest, true);
    }

    (String::new(), s, false)
}

#[cfg(windows)]
struct HandleGuard(HANDLE);

#[cfg(windows)]
impl Drop for HandleGuard {
    fn drop(&mut self) {
        if self.0 != INVALID_HANDLE_VALUE && !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
fn to_wide_null_str(s: &str) -> Vec<u16> {
    OsString::from(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn get_volume_mount_point(vol_root: &str) -> Option<String> {
    let wide = to_wide_null_str(vol_root);
    let mut needed: u32 = 0;
    let mut buf = vec![0u16; 260];
    let ok = unsafe {
        GetVolumePathNamesForVolumeNameW(
            wide.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        if unsafe { GetLastError() } == ERROR_MORE_DATA && needed > 0 {
            buf.resize(needed as usize, 0);
            let ok2 = unsafe {
                GetVolumePathNamesForVolumeNameW(
                    wide.as_ptr(),
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                    &mut needed,
                )
            };
            if ok2 == 0 {
                return None;
            }
        } else {
            return None;
        }
    }

    let mut candidates = Vec::new();
    let mut start = 0usize;
    while start < buf.len() {
        let Some(rel_end) = buf[start..].iter().position(|&c| c == 0) else {
            break;
        };
        if rel_end == 0 {
            break;
        }
        candidates.push(String::from_utf16_lossy(&buf[start..start + rel_end]));
        start += rel_end + 1;
    }

    if let Some(drive_root) = candidates.iter().find(|c| {
        let b = c.as_bytes();
        b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
    }) {
        return Some(drive_root.clone());
    }
    candidates.into_iter().next()
}

#[cfg(windows)]
fn query_dos_device_first(device_name: &str) -> Option<String> {
    let wide = to_wide_null_str(device_name);
    let mut buf = vec![0u16; 512];
    loop {
        let len = unsafe { QueryDosDeviceW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if len == 0 {
            if unsafe { GetLastError() } == ERROR_INSUFFICIENT_BUFFER && buf.len() < 32768 {
                buf.resize(buf.len() * 2, 0);
                continue;
            }
            return None;
        }
        let end = buf[..len as usize]
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(len as usize);
        if end == 0 {
            return None;
        }
        return Some(String::from_utf16_lossy(&buf[..end]));
    }
}

#[cfg(windows)]
fn resolve_nt_device_path(nt_path: &str) -> Option<String> {
    let mask = unsafe { GetLogicalDrives() };
    let mut best_match: Option<(char, String)> = None;

    for idx in 0..26u8 {
        if (mask & (1u32 << idx)) == 0 {
            continue;
        }
        let letter = (b'A' + idx) as char;
        let drive_dev = format!("{letter}:");
        let Some(target) = query_dos_device_first(&drive_dev) else {
            continue;
        };
        if !starts_with_ci_segment(&target, "\\Device") {
            continue;
        }
        let nb = nt_path.as_bytes();
        let tb = target.as_bytes();
        let matches = nb.len() >= tb.len()
            && nb[..tb.len()].eq_ignore_ascii_case(tb)
            && (nb.len() == tb.len() || nb[tb.len()] == b'\\' || nb[tb.len()] == b':');
        if matches {
            let replace_best = best_match
                .as_ref()
                .is_none_or(|(_, prev)| target.len() > prev.len());
            if replace_best {
                best_match = Some((letter, target));
            }
        }
    }

    let (letter, target) = best_match?;
    let mut tail = &nt_path[target.len()..];
    if tail.starts_with(':') {
        // Strip NTFS ADS attached directly to the device segment before appending remainder
        tail = match tail.find('\\') {
            Some(idx) => &tail[idx..],
            None => "",
        };
    }
    if tail.is_empty() {
        Some(format!("{letter}:\\"))
    } else {
        Some(format!("{letter}:{tail}"))
    }
}

#[cfg(windows)]
fn join_mount_and_tail(mount: &str, tail: &str) -> String {
    let clean_tail = tail.trim_start_matches('\\');
    if clean_tail.is_empty() {
        mount.to_string()
    } else {
        format!("{}\\{clean_tail}", mount.trim_end_matches('\\'))
    }
}

#[cfg(windows)]
fn call_final_path_by_handle(candidate: &str) -> Option<String> {
    let wide = to_wide_null_str(candidate);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return None;
    }
    let _guard = HandleGuard(handle);

    let mut buf = vec![0u16; 512];
    let mut len = unsafe {
        GetFinalPathNameByHandleW(handle, buf.as_mut_ptr(), buf.len() as u32, VOLUME_NAME_DOS)
    };
    if len == 0 {
        return None;
    }
    if (len as usize) >= buf.len() {
        buf.resize((len as usize) + 1, 0);
        len = unsafe {
            GetFinalPathNameByHandleW(handle, buf.as_mut_ptr(), buf.len() as u32, VOLUME_NAME_DOS)
        };
        if len == 0 || (len as usize) >= buf.len() {
            return None;
        }
    }

    let resolved = String::from_utf16_lossy(&buf[..len as usize]);
    if let Some(unc_tail) = strip_unc_namespace_prefix(&resolved) {
        return Some(format!("\\\\{unc_tail}"));
    }
    if let Some(drive_tail) = strip_drive_namespace_prefix(&resolved) {
        return Some(drive_tail.to_string());
    }
    None
}

#[cfg(windows)]
fn resolve_via_final_path_handle(raw_nt: &str) -> Option<String> {
    // Clean ADS from segments for metadata-only handle open while preserving non-existent tails
    let min_prefix_len = if let Some(rest) = raw_nt
        .strip_prefix("\\\\?\\")
        .or_else(|| raw_nt.strip_prefix("\\\\.\\"))
        .or_else(|| raw_nt.strip_prefix("\\??\\"))
    {
        let first_len = rest.find('\\').unwrap_or(rest.len());
        4 + first_len
    } else {
        raw_nt.len().min(8)
    };

    let mut current = raw_nt.trim_end_matches('\\').to_string();
    let mut tails: Vec<String> = Vec::new();

    for _ in 0..64 {
        let probe = if current.len() <= min_prefix_len {
            format!("{current}\\")
        } else {
            current.clone()
        };
        if let Some(resolved_base) = call_final_path_by_handle(&probe) {
            if tails.is_empty() {
                return Some(resolved_base);
            }
            tails.reverse();
            return Some(join_mount_and_tail(&resolved_base, &tails.join("\\")));
        }

        // Also try stripping NTFS ADS on the leaf segment at this level
        if let Some(slash_idx) = current.rfind('\\') {
            if slash_idx >= min_prefix_len {
                let leaf = &current[slash_idx + 1..];
                let clean_leaf = strip_ntfs_ads(leaf).trim_end_matches([' ', '.']);
                if clean_leaf != leaf && !clean_leaf.is_empty() {
                    let probe_clean = format!("{}\\{clean_leaf}", &current[..slash_idx]);
                    if let Some(resolved_base) = call_final_path_by_handle(&probe_clean) {
                        if tails.is_empty() {
                            return Some(resolved_base);
                        }
                        tails.reverse();
                        return Some(join_mount_and_tail(&resolved_base, &tails.join("\\")));
                    }
                }
                tails.push(leaf.to_string());
                current.truncate(slash_idx);
                continue;
            }
        }
        break;
    }

    None
}

#[cfg(windows)]
fn resolve_nt_or_volume_path_windows(replaced: &str, depth: u8) -> Option<String> {
    if depth > 4 {
        return None;
    }

    if let Some(rest) = replaced
        .strip_prefix("\\\\?\\")
        .or_else(|| replaced.strip_prefix("\\\\.\\"))
        .or_else(|| replaced.strip_prefix("\\??\\"))
    {
        let (first_raw, tail) = match rest.find('\\') {
            Some(idx) => (&rest[..idx], &rest[idx + 1..]),
            None => (rest, ""),
        };
        let first_clean = strip_ntfs_ads(first_raw).trim_end_matches([' ', '.']);

        // 1) Volume{GUID}
        if first_clean.len() >= 8
            && first_clean.as_bytes()[..7].eq_ignore_ascii_case(b"Volume{")
            && first_clean.ends_with('}')
        {
            let vol_root = format!("\\\\?\\{first_clean}\\");
            if let Some(mount) = get_volume_mount_point(&vol_root) {
                return Some(join_mount_and_tail(&mount, tail));
            }
            return resolve_via_final_path_handle(replaced);
        }

        // 2) GLOBALROOT\...
        if first_clean.eq_ignore_ascii_case("GLOBALROOT") {
            let nt_path = if tail.is_empty() {
                String::new()
            } else {
                format!("\\{tail}")
            };
            for nt_prefix in ["\\??\\", "\\DosDevices\\", "\\GLOBAL??\\"] {
                if nt_path.len() >= nt_prefix.len()
                    && nt_path.as_bytes()[..nt_prefix.len()]
                        .eq_ignore_ascii_case(nt_prefix.as_bytes())
                {
                    let inner = &nt_path[nt_prefix.len()..];
                    if inner
                        .as_bytes()
                        .get(..4)
                        .is_some_and(|h| h.eq_ignore_ascii_case(b"UNC\\"))
                    {
                        return Some(format!("\\\\{}", &inner[4..]));
                    }
                    if is_drive_only(inner) || starts_with_drive_root(inner) {
                        return Some(inner.to_string());
                    }
                    let wrapped = format!("\\\\?\\{inner}");
                    return resolve_nt_or_volume_path_windows(&wrapped, depth + 1);
                }
            }
            if starts_with_ci_segment(&nt_path, "\\Device") {
                if let Some(resolved) = resolve_nt_device_path(&nt_path) {
                    return Some(resolved);
                }
            }
            return resolve_via_final_path_handle(replaced);
        }

        // 3) Generic DOS device symlink (e.g. `BootPartition`, `SystemPartition`)
        if !first_clean.is_empty() {
            if let Some(target) = query_dos_device_first(first_clean) {
                if starts_with_ci_segment(&target, "\\Device") {
                    let nt_full = join_mount_and_tail(&target, tail);
                    if let Some(resolved) = resolve_nt_device_path(&nt_full) {
                        return Some(resolved);
                    }
                } else if target.len() >= 4
                    && target.as_bytes()[..4].eq_ignore_ascii_case(b"\\??\\")
                {
                    let inner = &target[4..];
                    if is_drive_only(inner) || starts_with_drive_root(inner) {
                        return Some(join_mount_and_tail(inner, tail));
                    }
                }
            }
            return resolve_via_final_path_handle(replaced);
        }

        return None;
    }

    // Direct NT paths without `\\.\GLOBALROOT` prefix
    for nt_prefix in ["\\DosDevices\\", "\\GLOBAL??\\"] {
        if replaced.len() >= nt_prefix.len()
            && replaced.as_bytes()[..nt_prefix.len()].eq_ignore_ascii_case(nt_prefix.as_bytes())
        {
            let inner = &replaced[nt_prefix.len()..];
            if is_drive_only(inner) || starts_with_drive_root(inner) {
                return Some(inner.to_string());
            }
            let wrapped = format!("\\\\?\\{inner}");
            return resolve_nt_or_volume_path_windows(&wrapped, depth + 1);
        }
    }

    if starts_with_ci_segment(replaced, "\\Device") {
        if let Some(resolved) = resolve_nt_device_path(replaced) {
            return Some(resolved);
        }
        let global_wrapped = format!("\\\\.\\GLOBALROOT{replaced}");
        return resolve_via_final_path_handle(&global_wrapped);
    }

    None
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

fn is_valid_deletable_target_norm(norm: &str) -> bool {
    let bytes = norm.as_bytes();
    if bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        return true;
    }
    if let Some(tail) = norm.strip_prefix("\\\\") {
        let mut parts = tail.split('\\').filter(|s| !s.is_empty());
        let server = parts.next();
        let share = parts.next();
        let subpath = parts.next();
        return server.is_some() && share.is_some() && subpath.is_some();
    }
    false
}

/// Returns true if the path is safe to consider for deletion (i.e. resolved to a rooted DOS/UNC
/// target, non-empty, not a drive/share root, and not a critical system path). Fails closed (`false`)
/// for unresolved NT/device/volume namespaces or unrooted paths.
pub fn is_deletable_path(path: &Path) -> bool {
    let norm = normalize_windows_path(path);
    if norm.is_empty() || is_drive_root_norm(&norm) || !is_valid_deletable_target_norm(&norm) {
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

    #[test]
    fn normalizes_non_ascii_prefixed_paths_without_utf8_slicing_panic() {
        assert_eq!(
            normalize_windows_path(Path::new(r"\\?\C:\Árbol")),
            r"c:\árbol"
        );
        assert_eq!(
            normalize_windows_path(Path::new(r"\\.\C:\Óscar")),
            r"c:\óscar"
        );
        assert_eq!(
            normalize_windows_path(Path::new(r"\??\D:\日本語")),
            r"d:\日本語"
        );
        assert_eq!(
            normalize_windows_path(Path::new(r"\\?\C:\Árbol\..")),
            r"c:\"
        );
    }

    #[test]
    fn blocks_ntfs_ads_index_allocation_and_consecutive_unc_backslash_bypasses() {
        assert_eq!(
            normalize_windows_path(Path::new(r"C:\::$INDEX_ALLOCATION")),
            r"c:\"
        );
        assert_eq!(
            normalize_windows_path(Path::new(r"D:\::$INDEX_ALLOCATION")),
            r"d:\"
        );
        assert_eq!(
            normalize_windows_path(Path::new(r"C:\Windows::$INDEX_ALLOCATION")),
            r"c:\windows"
        );
        assert_eq!(
            normalize_windows_path(Path::new(r"\\.\C:\Windows::$INDEX_ALLOCATION")),
            r"c:\windows"
        );
        assert_eq!(
            normalize_windows_path(Path::new(r"C:\Program Files::$INDEX_ALLOCATION")),
            r"c:\program files"
        );

        for critical_case in [
            r"C:\::$INDEX_ALLOCATION",
            r"C:\Windows::$INDEX_ALLOCATION",
            r"\\.\C:\Windows::$INDEX_ALLOCATION",
            r"C:\Program Files::$INDEX_ALLOCATION",
            r"\\localhost\\C$\Windows",
            r"\\?\UNC\localhost\\C$\Windows",
            r"\\localhost\C$::$INDEX_ALLOCATION\Windows",
        ] {
            assert!(
                is_critical_system_path(Path::new(critical_case)),
                "expected critical system path for {critical_case}"
            );
            assert!(
                !is_deletable_path(Path::new(critical_case)),
                "expected non-deletable path for {critical_case}"
            );
        }

        assert!(is_drive_root(Path::new(r"D:\::$INDEX_ALLOCATION")));
        assert!(!is_deletable_path(Path::new(r"D:\::$INDEX_ALLOCATION")));

        if Path::new(r"C:\PROGRA~1").exists() {
            assert_eq!(
                normalize_windows_path(Path::new(r"C:\PROGRA~1::$INDEX_ALLOCATION")),
                r"c:\program files"
            );
            assert!(is_critical_system_path(Path::new(
                r"C:\PROGRA~1::$INDEX_ALLOCATION"
            )));
            assert!(!is_deletable_path(Path::new(
                r"C:\PROGRA~1::$INDEX_ALLOCATION"
            )));
        }
    }

    #[test]
    fn resolves_and_protects_nt_device_and_volume_guid_namespaces() {
        use windows_sys::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;

        // Discover C:\ Volume GUID dynamically
        let c_root_w = to_wide_null_str(r"C:\");
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
        let vol_no_slash = vol_root.trim_end_matches('\\');

        let vol_win = format!("{vol_root}Windows");
        let vol_prog = format!("{vol_root}Program Files");
        let vol_ads = format!("{vol_root}Windows::$INDEX_ALLOCATION");

        assert_eq!(normalize_windows_path(Path::new(&vol_root)), r"c:\");
        assert_eq!(normalize_windows_path(Path::new(vol_no_slash)), r"c:\");
        assert_eq!(normalize_windows_path(Path::new(&vol_win)), r"c:\windows");
        assert_eq!(
            normalize_windows_path(Path::new(&vol_prog)),
            r"c:\program files"
        );
        assert_eq!(normalize_windows_path(Path::new(&vol_ads)), r"c:\windows");

        assert!(is_drive_root(Path::new(&vol_root)));
        assert!(is_drive_root(Path::new(vol_no_slash)));

        for protected_vol in [&vol_root, &vol_win, &vol_prog, &vol_ads] {
            assert!(
                is_critical_system_path(Path::new(protected_vol)),
                "expected critical system path for {protected_vol}"
            );
            assert!(
                !is_deletable_path(Path::new(protected_vol)),
                "expected non-deletable path for {protected_vol}"
            );
        }

        // Discover C: NT device path dynamically via QueryDosDeviceW
        let nt_dev = query_dos_device_first("C:").expect("QueryDosDeviceW(C:) must succeed");
        let globalroot_dev_root = format!(r"\\.\GLOBALROOT{nt_dev}\");
        let globalroot_dev_win = format!(r"\\.\GLOBALROOT{nt_dev}\Windows");
        let globalroot_ext_win = format!(r"\\?\GLOBALROOT{nt_dev}\Windows");
        let globalroot_nt_win = format!(r"\??\GLOBALROOT{nt_dev}\Windows");
        let direct_dev_win = format!(r"{nt_dev}\Windows");

        assert_eq!(
            normalize_windows_path(Path::new(&globalroot_dev_root)),
            r"c:\"
        );
        assert!(is_drive_root(Path::new(&globalroot_dev_root)));

        for nt_case in [
            globalroot_dev_root,
            globalroot_dev_win,
            globalroot_ext_win,
            globalroot_nt_win,
            direct_dev_win,
        ] {
            assert!(
                is_critical_system_path(Path::new(&nt_case)),
                "expected critical system path for {nt_case}"
            );
            assert!(
                !is_deletable_path(Path::new(&nt_case)),
                "expected non-deletable path for {nt_case}"
            );
        }

        if query_dos_device_first("BootPartition").is_some() {
            assert!(is_critical_system_path(Path::new(
                r"\\.\BootPartition\Windows"
            )));
            assert!(!is_deletable_path(Path::new(r"\\.\BootPartition\Windows")));
            assert!(!is_deletable_path(Path::new(r"\\.\BootPartition\")));
        }

        // Verify metadata-only CreateFileW + GetFinalPathNameByHandleW fallback (including non-existent tail)
        let handle_resolved_existing = resolve_via_final_path_handle(&vol_win)
            .expect("resolve_via_final_path_handle must resolve existing Volume GUID Windows path");
        assert_eq!(handle_resolved_existing.to_ascii_lowercase(), r"c:\windows");

        let vol_nonexistent = format!("{vol_win}\\NonExistentSub\\Leaf.txt");
        let handle_resolved_ancestor = resolve_via_final_path_handle(&vol_nonexistent)
            .expect("resolve_via_final_path_handle must walk up to existing ancestor");
        assert_eq!(
            handle_resolved_ancestor.to_ascii_lowercase(),
            r"c:\windows\nonexistentsub\leaf.txt"
        );

        // Fail-closed guard for unresolved NT / Volume / Device namespaces
        for unresolved in [
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\Windows",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\Temp\file.txt",
            r"\\.\GLOBALROOT\Device\HarddiskVolume99\Windows",
            r"\\.\GLOBALROOT\Device\HarddiskVolume99\Temp\file.txt",
            r"\\.\PhysicalDrive0",
            r"\Device\HarddiskVolume99\Temp\file.txt",
            r"\\.\Pipe\nexora_nonexistent",
        ] {
            assert!(
                !is_deletable_path(Path::new(unresolved)),
                "unresolved device/volume namespace must fail closed: {unresolved}"
            );
        }
    }

    #[test]
    fn blocks_local_hostname_loopback_range_and_ipv6_literal_unc_admin_shares() {
        let mut unc_cases = vec![
            String::from(r"\\127.0.0.2\C$\Windows"),
            String::from(r"\\127.255.255.254\C$\Windows"),
            String::from(r"\\[::1]\C$\Windows"),
            String::from(r"\\0:0:0:0:0:0:0:1\C$\Windows"),
            String::from(r"\\0--1.ipv6-literal.net\C$\Windows"),
            String::from(r"\\0--0--0--0--0--0--0--1.ipv6-literal.net\C$\Windows"),
            String::from(r"\\?\UNC\0--1.ipv6-literal.net\C$\Windows"),
            String::from(r"\\?\Unc\127.0.0.2\C$\Windows"),
            String::from(r"\\.\UNC\0--1.ipv6-literal.net\C$\Program Files"),
            String::from(r"\??\UNC\127.0.0.2\C$\ProgramData"),
        ];

        if let Ok(comp) = std::env::var("COMPUTERNAME") {
            let clean = comp.trim();
            if !clean.is_empty() {
                unc_cases.push(format!(r"\\{clean}\C$\Windows"));
                unc_cases.push(format!(r"\\?\UNC\{clean}\C$\Windows"));
                unc_cases.push(format!(r"\\{clean}.\C$\Program Files"));
                unc_cases.push(format!(r"\\{clean}\C$"));
            }
        }
        if let Some(dns_fqdn) = query_computer_name(ComputerNameDnsFullyQualified) {
            unc_cases.push(format!(r"\\{dns_fqdn}\C$\Windows"));
        }

        for unc_case in &unc_cases {
            assert!(
                is_critical_system_path(Path::new(unc_case)),
                "expected critical system path for {unc_case}"
            );
            assert!(
                !is_deletable_path(Path::new(unc_case)),
                "expected non-deletable path for {unc_case}"
            );
        }

        // Non-system drive share on local host maps to D:\ (drive root not deletable, child deletable)
        assert!(!is_deletable_path(Path::new(
            r"\\0--1.ipv6-literal.net\D$\"
        )));
        assert!(is_deletable_path(Path::new(
            r"\\0--1.ipv6-literal.net\D$\Data"
        )));
        // Bare remote UNC share root is not deletable
        assert!(!is_deletable_path(Path::new(r"\\RemoteServer\Share")));
        assert!(is_deletable_path(Path::new(r"\\RemoteServer\Share\Folder")));
    }

    #[test]
    fn resolves_relative_and_root_relative_paths_safely_without_mutating_cwd() {
        // Non-mutating base-dir checks (simulates CWD = C:\ and CWD = C:\Windows)
        let norm_win_from_c = normalize_windows_path_with_base("Windows", Some(Path::new(r"C:\")));
        assert_eq!(norm_win_from_c, r"c:\windows");
        assert!(is_critical_system_path_norm(&norm_win_from_c));

        let norm_sys32_from_win =
            normalize_windows_path_with_base("System32", Some(Path::new(r"C:\Windows")));
        assert_eq!(norm_sys32_from_win, r"c:\windows\system32");
        assert!(is_critical_system_path_norm(&norm_sys32_from_win));

        let norm_dot_from_c = normalize_windows_path_with_base(".", Some(Path::new(r"C:\")));
        assert_eq!(norm_dot_from_c, r"c:\");
        assert!(is_drive_root_norm(&norm_dot_from_c));

        // Direct public API checks using current process CWD (no env::set_current_dir race)
        let cwd = std::env::current_dir().expect("current_dir");
        assert_eq!(
            normalize_windows_path(Path::new(".")),
            normalize_windows_path(&cwd)
        );
        assert!(!is_deletable_path(Path::new("")));
        assert!(!is_deletable_path(Path::new("   ")));
        assert!(!is_deletable_path(Path::new(r"\")));
        assert!(is_critical_system_path(Path::new(r"\Windows")));
        assert!(!is_deletable_path(Path::new(r"\Windows")));
    }
}
