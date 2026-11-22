//! WinRT toast notifications with the app's own AUMID (registered under HKCU, no admin, no Start-menu shortcut).
//! Release builds only (same gate as autostart: HKCU is user state a sandboxed LOCALAPPDATA does not cover);
//! anything that fails here makes the caller fall back to the tray balloon.

use windows::core::HSTRING;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_SINGLETHREADED};
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows_sys::Win32::System::Registry::*;

pub const AUMID: &str = "ClaudeUsage.App";
const KEY: &str = r"Software\Classes\AppUserModelId\ClaudeUsage.App";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn set_sz(key: HKEY, name: &str, val: &str) -> bool {
    let (n, v) = (wide(name), wide(val));
    // SAFETY: NUL-terminated UTF-16, length in bytes including the terminator.
    unsafe {
        RegSetValueExW(
            key,
            n.as_ptr(),
            0,
            REG_SZ,
            v.as_ptr() as *const u8,
            (v.len() * 2) as u32,
        ) == 0
    }
}

/// Create/refresh `HKCU\Software\Classes\AppUserModelId\ClaudeUsage.App` (DisplayName + IconUri when an
/// `icon.png` sits next to the exe). Returns false when the registry write failed.
pub fn register(exe: &std::path::Path) -> bool {
    // SAFETY: plain registry calls; the key is closed.
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(KEY).as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        ) != 0
        {
            return false;
        }
        let mut ok = set_sz(key, "DisplayName", "Claude usage");
        let icon = exe.with_file_name("icon.png");
        if icon.exists() {
            ok &= set_sz(key, "IconUri", &icon.display().to_string());
        }
        RegCloseKey(key);
        ok
    }
}

pub fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            _ => o.push(c),
        }
    }
    o
}

pub fn toast_xml(title: &str, body: &str) -> String {
    format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_escape(title),
        xml_escape(body)
    )
}

/// Show a toast. Must be called from a thread that may initialise WinRT (the tray UI thread). Err = use the balloon.
pub fn show(title: &str, body: &str) -> Result<(), String> {
    let run = || -> windows::core::Result<()> {
        // already-initialised / different-mode results are fine: WinRT is usable either way
        let _ = unsafe { RoInitialize(RO_INIT_SINGLETHREADED) };
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(toast_xml(title, body)))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)
    };
    run().map_err(|e| format!("{:#x}", e.code().0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_is_escaped() {
        let x = toast_xml("a<b", "x & \"y\" 'z' > ü");
        assert!(x.contains("<text>a&lt;b</text>"));
        assert!(x.contains("x &amp; &quot;y&quot; &apos;z&apos; &gt; ü"));
    }

    #[test]
    fn xml_parses_as_a_toast_document() {
        let doc = XmlDocument::new().unwrap();
        doc.LoadXml(&HSTRING::from(toast_xml(
            "Claude usage - T01 80%",
            "5h <80%> & 7d",
        )))
        .unwrap();
    }
}
