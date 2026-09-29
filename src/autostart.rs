//! "Start with Windows" via the per-user Run key.
//!
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` needs no elevation and
//! affects only the current user — the right scope for a tool teammates each
//! opt into individually.

use std::path::Path;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_NONE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};

pub const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const VALUE_NAME: &str = "ClaudeUsageWidget";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_run_key(subkey_path: &str) -> std::io::Result<HKEY> {
    let path = wide(subkey_path);
    let mut hkey = HKEY::default();
    // RegCreateKeyExW opens the key if it already exists, so this doubles as
    // the read/delete path too — the Run key always exists in practice.
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut hkey,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status.0 as i32));
    }
    Ok(hkey)
}

/// Point the value at `exe_path` (quoted, since install paths may contain
/// spaces) so Windows launches it at logon.
pub fn enable(subkey_path: &str, value_name: &str, exe_path: &Path) -> std::io::Result<()> {
    let hkey = open_run_key(subkey_path)?;
    let quoted = format!("\"{}\"", exe_path.display());
    let data = wide(&quoted);
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2) };

    let name = wide(value_name);
    let status =
        unsafe { RegSetValueExW(hkey, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes)) };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status.0 as i32));
    }
    Ok(())
}

/// Remove the value. Not being present at all is success, not an error —
/// disabling an already-disabled autostart should never surface a failure.
pub fn disable(subkey_path: &str, value_name: &str) -> std::io::Result<()> {
    let hkey = open_run_key(subkey_path)?;
    let name = wide(value_name);
    let status = unsafe { RegDeleteValueW(hkey, PCWSTR(name.as_ptr())) };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    match status {
        s if s == ERROR_SUCCESS => Ok(()),
        s if s.0 == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.0 => Ok(()),
        s => Err(std::io::Error::from_raw_os_error(s.0 as i32)),
    }
}

/// The currently registered command line, if any. Read-path counterpart to
/// `enable`/`disable`, exercised by tests now; the About panel (M4) and
/// startup diagnostics are its intended production callers.
#[allow(dead_code)]
pub fn current_value(subkey_path: &str, value_name: &str) -> Option<String> {
    let path = wide(subkey_path);
    let name = wide(value_name);
    let mut buf = [0u16; 1024];
    let mut buf_len = (buf.len() * 2) as u32;
    let mut value_type = REG_NONE;

    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            Some(&mut value_type),
            Some(buf.as_mut_ptr().cast()),
            Some(&mut buf_len),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let chars = (buf_len as usize / 2).saturating_sub(1); // drop the null terminator
    Some(String::from_utf16_lossy(&buf[..chars]))
}

#[allow(dead_code)]
pub fn is_enabled(subkey_path: &str, value_name: &str) -> bool {
    current_value(subkey_path, value_name).is_some()
}

/// Reconcile the registry with `Settings::start_with_windows`. Best-effort: a
/// failure here is logged, never fatal to the widget.
pub fn apply(enabled: bool, exe_path: &Path) {
    let result = if enabled {
        enable(RUN_KEY_PATH, VALUE_NAME, exe_path)
    } else {
        disable(RUN_KEY_PATH, VALUE_NAME)
    };
    if let Err(e) = result {
        log::warn!("could not update autostart registry value: {e}");
    }
}

#[allow(dead_code)]
fn unused_pwstr_hint(_: Option<PWSTR>) {}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway subkey under HKCU so tests never touch the real Run key.
    // Each test gets its OWN value name: cargo runs tests in parallel threads
    // within one process, and a shared registry value raced under that.
    const TEST_SUBKEY: &str = r"Software\ClaudeUsageWidgetTests\Run";

    fn cleanup(value_name: &str) {
        let _ = disable(TEST_SUBKEY, value_name);
    }

    #[test]
    fn enabling_then_reading_round_trips_the_path() {
        let value_name = "RoundTrip";
        cleanup(value_name);
        let exe = Path::new(r"C:\Users\test\claude-usage-widget.exe");
        enable(TEST_SUBKEY, value_name, exe).expect("enable should succeed under HKCU");

        assert!(is_enabled(TEST_SUBKEY, value_name));
        let value = current_value(TEST_SUBKEY, value_name).unwrap();
        assert!(value.contains("claude-usage-widget.exe"));
        assert!(value.starts_with('"'), "path should be quoted: {value}");

        cleanup(value_name);
    }

    #[test]
    fn disabling_removes_the_value() {
        let value_name = "DisableRemoves";
        cleanup(value_name);
        let exe = Path::new(r"C:\Users\test\claude-usage-widget.exe");
        enable(TEST_SUBKEY, value_name, exe).unwrap();
        assert!(is_enabled(TEST_SUBKEY, value_name));

        disable(TEST_SUBKEY, value_name).unwrap();
        assert!(!is_enabled(TEST_SUBKEY, value_name));
    }

    #[test]
    fn disabling_an_already_absent_value_is_not_an_error() {
        let value_name = "AlreadyAbsent";
        cleanup(value_name);
        assert!(disable(TEST_SUBKEY, value_name).is_ok());
    }

    #[test]
    fn missing_value_reads_as_disabled() {
        let value_name = "NeverWritten";
        cleanup(value_name);
        assert!(!is_enabled(TEST_SUBKEY, value_name));
        assert!(current_value(TEST_SUBKEY, value_name).is_none());
    }
}
