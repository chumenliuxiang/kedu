use crate::domain::ActivityIdentity;

/// A sound-only completion alert must not create a notification or activate UI.
#[cfg(target_os = "windows")]
pub fn play_focus_completion_sound() {
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(
            windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK,
        );
    }
}

#[cfg(not(target_os = "windows"))]
pub fn play_focus_completion_sound() {}

#[cfg(target_os = "windows")]
mod icon;

#[cfg(target_os = "windows")]
mod applications;

#[cfg(target_os = "windows")]
mod application_identity;

#[cfg(target_os = "windows")]
pub use icon::{application_icon_data_url, foreground_application_icon};

#[cfg(target_os = "windows")]
pub use applications::{application_from_path, file_display_name, installed_applications};

#[cfg(not(target_os = "windows"))]
pub fn file_display_name(_path: &str) -> Option<String> {
    None
}

#[cfg(not(target_os = "windows"))]
pub fn application_icon_data_url(_path: &str) -> Option<String> {
    None
}

#[cfg(not(target_os = "windows"))]
pub fn foreground_application_icon(_path: &str) -> Option<String> {
    None
}

#[cfg(not(target_os = "windows"))]
pub fn installed_applications() -> Vec<crate::domain::TargetApplication> {
    Vec::new()
}

#[cfg(not(target_os = "windows"))]
pub fn application_from_path(
    _path: &std::path::Path,
) -> Result<crate::domain::TargetApplication, String> {
    Err("当前系统不支持选择 Windows 应用程序".into())
}

#[cfg(target_os = "windows")]
pub fn foreground_activity(idle_threshold_seconds: u64) -> Option<ActivityIdentity> {
    use std::path::Path;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    };

    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            return None;
        }

        let title_len = GetWindowTextLengthW(window);
        let mut title_buffer = vec![0u16; (title_len.max(0) + 1) as usize];
        let copied = GetWindowTextW(window, title_buffer.as_mut_ptr(), title_buffer.len() as i32);
        let title = String::from_utf16_lossy(&title_buffer[..copied.max(0) as usize]);

        let process_path = application_identity::window_process_path(window)?;
        // Resolution may race an application switch or a hosted window closing.
        if GetForegroundWindow() != window {
            return None;
        }

        let app_name =
            application_identity::packaged_display_name(&process_path).unwrap_or_else(|| {
                Path::new(&process_path)
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty())
                    .unwrap_or("未知应用")
                    .to_string()
            });

        let idle_seconds = idle_seconds();
        Some(ActivityIdentity {
            app_name,
            process_path,
            title,
            idle: idle_seconds >= idle_threshold_seconds,
            idle_seconds,
            idle_exempt: false,
        })
    }
}

#[cfg(target_os = "windows")]
pub fn idle_seconds() -> u64 {
    use std::mem::size_of;
    use windows_sys::Win32::{
        System::SystemInformation::GetTickCount,
        UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
    };

    unsafe {
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if GetLastInputInfo(&mut info) == 0 {
            return 0;
        }
        GetTickCount().wrapping_sub(info.dwTime) as u64 / 1000
    }
}

#[cfg(target_os = "windows")]
pub fn foreground_window_id() -> isize {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
}

/// A policy query may outlive the foreground window it inspected.
pub fn foreground_matches(expected: isize) -> bool {
    expected != 0 && foreground_window_id() == expected
}

/// Opt-in legacy mode. Never modify geometry, enable state, or restore a window.
#[cfg(target_os = "windows")]
pub fn minimize_restricted_foreground(expected: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindowAsync, SW_MINIMIZE};
    if foreground_matches(expected) && !is_essential_foreground() {
        unsafe {
            ShowWindowAsync(expected as _, SW_MINIMIZE);
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn minimize_restricted_foreground(_expected: isize) {}

#[cfg(target_os = "windows")]
pub fn is_essential_foreground() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId,
    };
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            return true;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        if pid == std::process::id() {
            return true;
        }
        let mut class = [0u16; 128];
        let count = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
        let class = String::from_utf16_lossy(&class[..count.max(0) as usize]);
        let path = application_identity::window_process_path(window).unwrap_or_default();
        let windows_dir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        is_essential_shell_window(&class, &path, &windows_dir)
    }
}

/// Explorer owns both real folder windows and desktop/taskbar infrastructure.
/// Minimizing an internal shell or XAML host window can corrupt its layout even
/// after focus ends; only actual Explorer folder frames should be guarded.
#[cfg(any(target_os = "windows", test))]
fn is_essential_shell_window(class: &str, process_path: &str, windows_dir: &str) -> bool {
    if matches!(
        class,
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "MultitaskingViewFrame"
            | "XamlExplorerHostIslandWindow"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
    ) {
        return true;
    }
    let path = process_path.replace('/', "\\");
    let explorer = format!(
        "{}\\explorer.exe",
        windows_dir.trim_end_matches(['\\', '/'])
    );
    path.eq_ignore_ascii_case(&explorer) && !matches!(class, "CabinetWClass" | "ExploreWClass")
}

#[cfg(target_os = "windows")]
pub fn watch_foreground(sender: std::sync::mpsc::SyncSender<()>) {
    use std::sync::{mpsc::SyncSender, OnceLock};
    use windows_sys::Win32::{
        Foundation::HWND,
        UI::{
            Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
            WindowsAndMessaging::{
                DispatchMessageW, GetMessageW, TranslateMessage, EVENT_SYSTEM_FOREGROUND, MSG,
                WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
            },
        },
    };
    static SIGNAL: OnceLock<SyncSender<()>> = OnceLock::new();
    if SIGNAL.set(sender).is_err() {
        return;
    }
    unsafe extern "system" fn changed(
        _: HWINEVENTHOOK,
        _: u32,
        _: HWND,
        _: i32,
        _: i32,
        _: u32,
        _: u32,
    ) {
        // The OS callback must stay fast and non-reentrant: coalesce only.
        if let Some(sender) = SIGNAL.get() {
            let _ = sender.try_send(());
        }
    }
    std::thread::spawn(|| unsafe {
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            std::ptr::null_mut(),
            Some(changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        if hook.is_null() {
            eprintln!("foreground hook unavailable; periodic guard remains active");
            return;
        }
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        UnhookWinEvent(hook);
    });
}

#[cfg(not(target_os = "windows"))]
pub fn foreground_activity(_idle_threshold_seconds: u64) -> Option<ActivityIdentity> {
    None
}

#[cfg(not(target_os = "windows"))]
pub fn foreground_window_id() -> isize {
    0
}

#[cfg(not(target_os = "windows"))]
pub fn is_essential_foreground() -> bool {
    true
}

#[cfg(not(target_os = "windows"))]
pub fn watch_foreground(_sender: std::sync::mpsc::SyncSender<()>) {}

#[cfg(not(target_os = "windows"))]
pub fn idle_seconds() -> u64 {
    0
}

#[cfg(test)]
mod shell_guard_tests {
    use super::is_essential_shell_window;

    #[test]
    fn protects_explorer_infrastructure_without_exempting_folder_frames() {
        let windows = "C:\\Windows";
        let explorer = "c:\\windows\\explorer.exe";
        for class in [
            "NotifyIconOverflowWindow",
            "TopLevelWindowForOverflowXamlIsland",
            "Windows.UI.Core.CoreWindow",
            "XamlExplorerHostIslandWindow",
            "Shell_TrayWnd",
        ] {
            assert!(
                is_essential_shell_window(class, explorer, windows),
                "{class}"
            );
        }
        for class in ["CabinetWClass", "ExploreWClass"] {
            assert!(
                !is_essential_shell_window(class, explorer, windows),
                "{class}"
            );
        }
        assert!(!is_essential_shell_window(
            "Chrome_WidgetWin_1",
            "C:\\Apps\\browser.exe",
            windows
        ));
        assert!(!is_essential_shell_window(
            "Windows.UI.Core.CoreWindow",
            "C:\\Apps\\explorer.exe",
            windows
        ));
    }
}
