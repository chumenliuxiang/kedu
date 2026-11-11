//! Resolve the content process of a hosted Store window, not its shared frame.
use std::{
    collections::HashMap,
    io::Read,
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use quick_xml::{events::Event, Reader, XmlVersion};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM},
    System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    },
    UI::WindowsAndMessaging::{
        EnumChildWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
    },
};

fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buffer = vec![0u16; 32_768];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length);
        CloseHandle(process);
        (ok != 0).then(|| String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

fn is_frame_host(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("ApplicationFrameHost.exe"))
}

fn unique_content_pid(host: u32, candidates: &[u32]) -> Option<u32> {
    let mut content = None;
    for &pid in candidates {
        if pid == 0 || pid == host {
            continue;
        }
        if content.is_some_and(|previous| previous != pid) {
            return None;
        }
        content = Some(pid);
    }
    content
}

pub(super) fn window_process_path(window: HWND) -> Option<String> {
    if window.is_null() {
        return None;
    }
    unsafe {
        let mut host = 0;
        GetWindowThreadProcessId(window, &mut host);
        let path = process_path(host)?;
        if !is_frame_host(&path) {
            return Some(path);
        }
        unsafe extern "system" fn collect(child: HWND, parameter: LPARAM) -> i32 {
            let mut class = [0u16; 128];
            let length = GetClassNameW(child, class.as_mut_ptr(), class.len() as i32);
            if IsWindowVisible(child) != 0
                && String::from_utf16_lossy(&class[..length.max(0) as usize])
                    == "Windows.UI.Core.CoreWindow"
            {
                let mut pid = 0;
                GetWindowThreadProcessId(child, &mut pid);
                (*(parameter as *mut Vec<u32>)).push(pid);
            }
            1
        }
        let mut candidates = Vec::<u32>::new();
        EnumChildWindows(window, Some(collect), &mut candidates as *mut _ as LPARAM);
        // Never guess from a title, other top-level windows, or the first process
        // with a matching name. During activation an app may have no content yet.
        let content = process_path(unique_content_pid(host, &candidates)?)?;
        (!is_frame_host(&content)).then_some(content)
    }
}

fn literal_name(name: String) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && !name.to_ascii_lowercase().starts_with("ms-resource:"))
        .then(|| name.to_string())
}

fn manifest_display_name(xml: &str, executable: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut matched = false;
    let mut version = XmlVersion::Implicit1_0;
    loop {
        match reader.read_event().ok()? {
            Event::Decl(declaration) => version = declaration.xml_version().ok()?,
            Event::Start(element) | Event::Empty(element) => {
                if element.local_name().as_ref() == b"Application" {
                    matched = false;
                    for attribute in element.attributes() {
                        let attribute = attribute.ok()?;
                        if attribute.key.as_ref() == b"Executable" {
                            let value = attribute
                                .decoded_and_normalized_value(version, reader.decoder())
                                .ok()?;
                            matched = value.replace('/', "\\").eq_ignore_ascii_case(executable);
                        }
                    }
                } else if matched && element.local_name().as_ref() == b"VisualElements" {
                    for attribute in element.attributes() {
                        let attribute = attribute.ok()?;
                        if attribute.key.as_ref() == b"DisplayName" {
                            return literal_name(
                                attribute
                                    .decoded_and_normalized_value(version, reader.decoder())
                                    .ok()?
                                    .into_owned(),
                            );
                        }
                    }
                }
            }
            Event::End(element) if element.local_name().as_ref() == b"Application" => {
                matched = false
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

fn read_packaged_name(path: &str) -> Option<String> {
    let executable = Path::new(path);
    // Store packages can be installed on other drives; do not hardcode C:.
    if !executable
        .components()
        .any(|part| part.as_os_str().eq_ignore_ascii_case("WindowsApps"))
    {
        return None;
    }
    for directory in executable.ancestors().skip(1).take(6) {
        if directory
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("WindowsApps"))
        {
            break;
        }
        let manifest = directory.join("AppxManifest.xml");
        let Ok(file) = std::fs::File::open(manifest) else {
            continue;
        };
        if file.metadata().ok()?.len() > 1_048_576 {
            return None;
        }
        let mut xml = String::new();
        file.take(1_048_577).read_to_string(&mut xml).ok()?;
        if xml.len() > 1_048_576 {
            return None;
        }
        return manifest_display_name(&xml, executable.strip_prefix(directory).ok()?.to_str()?);
    }
    None
}

pub(super) fn packaged_display_name(path: &str) -> Option<String> {
    type Entry = (Instant, Option<String>);
    static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = path.to_lowercase();
    if let Ok(entries) = cache.lock() {
        if let Some((at, name)) = entries.get(&key) {
            if at.elapsed() < Duration::from_secs(if name.is_some() { 600 } else { 60 }) {
                return name.clone();
            }
        }
    }
    let name = read_packaged_name(path);
    if let Ok(mut entries) = cache.lock() {
        if entries.len() >= 128 {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.0)
                .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(key, (Instant::now(), name.clone()));
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_process_requires_one_distinct_content_owner() {
        assert_eq!(unique_content_pid(10, &[10, 0, 20, 20]), Some(20));
        assert_eq!(unique_content_pid(10, &[10, 0]), None);
        assert_eq!(unique_content_pid(10, &[20, 30]), None);
        assert!(is_frame_host(
            r"C:\Windows\System32\APPLICATIONFRAMEHOST.EXE"
        ));
        assert!(!is_frame_host(r"C:\Apps\ApplicationFrameHost.exe.backup"));
        assert!(!is_frame_host(r"C:\Apps\Db.App.exe"));
    }

    #[test]
    fn manifest_uses_matching_executable_and_decodes_name() {
        let xml = r#"<Package><Applications><Application Executable="Other.exe"><uap:VisualElements DisplayName="Other"/></Application><Application Executable="bin\Db.App.exe"><uap:VisualElements DisplayName="PDF &amp; Notes"/></Application></Applications></Package>"#;
        assert_eq!(
            manifest_display_name(xml, r"BIN\DB.APP.EXE"),
            Some("PDF & Notes".into())
        );
        assert_eq!(manifest_display_name(xml, "Missing.exe"), None);
        assert_eq!(
            manifest_display_name(
                &xml.replace("PDF &amp; Notes", "ms-resource:AppName"),
                r"bin\Db.App.exe"
            ),
            None
        );
        assert_eq!(manifest_display_name("<broken", "Db.App.exe"), None);
    }

    #[test]
    fn ordinary_executables_and_null_windows_are_safe() {
        assert!(packaged_display_name(r"C:\Missing\Editor.exe").is_none());
        assert!(window_process_path(std::ptr::null_mut()).is_none());
    }

    #[test]
    #[ignore = "Read-only check requiring an open Drawboard PDF window"]
    fn live_drawboard_host_resolves_actual_application() {
        use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;
        unsafe extern "system" fn collect(window: HWND, data: LPARAM) -> i32 {
            let mut class = [0u16; 128];
            let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
            if String::from_utf16_lossy(&class[..length.max(0) as usize])
                == "ApplicationFrameWindow"
            {
                if let Some(path) = window_process_path(window) {
                    if packaged_display_name(&path).as_deref() == Some("Drawboard PDF") {
                        (*(data as *mut Vec<String>)).push(path);
                    }
                }
            }
            1
        }
        let mut resolved = Vec::<String>::new();
        unsafe {
            EnumWindows(Some(collect), &mut resolved as *mut _ as LPARAM);
        }
        assert!(
            !resolved.is_empty(),
            "Open Drawboard PDF without minimizing it before this check"
        );
        for path in &resolved {
            assert!(!is_frame_host(path));
            assert!(Path::new(path).is_file());
        }
        println!(
            "Drawboard PDF resolved: {}",
            serde_json::to_string(&resolved).unwrap()
        );
    }
}
