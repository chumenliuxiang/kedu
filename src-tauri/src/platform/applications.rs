use std::{collections::BTreeMap, os::windows::process::CommandExt, path::Path, process::Command};

use winreg::{
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY},
    RegKey,
};

use crate::domain::TargetApplication;

/// Read Windows executable metadata once when the catalog is built, not per poll.
pub fn file_display_name(path: &str) -> Option<String> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    let wide: Vec<u16> = std::ffi::OsStr::new(path)
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        let size = GetFileVersionInfoSizeW(wide.as_ptr(), ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut translations: *mut c_void = ptr::null_mut();
        let mut length = 0u32;
        let key: Vec<u16> = "\\VarFileInfo\\Translation\0".encode_utf16().collect();
        let mut locales = Vec::new();
        if VerQueryValueW(
            data.as_ptr().cast(),
            key.as_ptr(),
            &mut translations,
            &mut length,
        ) != 0
            && length >= 4
            && !translations.is_null()
        {
            let words = std::slice::from_raw_parts(translations.cast::<u16>(), length as usize / 2);
            locales = words
                .chunks_exact(2)
                .map(|pair| (pair[0], pair[1]))
                .collect();
        }
        locales.extend([(0x0804, 0x04b0), (0x0409, 0x04b0)]);
        for field in ["FileDescription", "ProductName"] {
            for (language, codepage) in &locales {
                let query: Vec<u16> =
                    format!("\\StringFileInfo\\{language:04x}{codepage:04x}\\{field}\0")
                        .encode_utf16()
                        .collect();
                let mut value: *mut c_void = ptr::null_mut();
                let mut count = 0u32;
                if VerQueryValueW(data.as_ptr().cast(), query.as_ptr(), &mut value, &mut count) != 0
                    && count > 0
                    && !value.is_null()
                {
                    let text = String::from_utf16_lossy(std::slice::from_raw_parts(
                        value.cast::<u16>(),
                        count as usize,
                    ));
                    let name = text.trim_end_matches('\0').trim();
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    None
}

fn executable_path(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"');
    let lower = value.to_lowercase();
    let end = lower.find(".exe").map(|index| index + 4)?;
    let path = value[..end].trim().trim_matches('"');
    let path = std::env::vars().fold(path.to_string(), |current, (name, value)| {
        current.replace(&format!("%{name}%"), &value)
    });
    Path::new(&path)
        .is_file()
        .then(|| Path::new(&path).to_string_lossy().into_owned())
}

fn application(display_name: String, path: String) -> Option<TargetApplication> {
    let executable_name = Path::new(&path).file_name()?.to_str()?.to_string();
    let lowered = format!("{} {}", display_name, executable_name).to_lowercase();
    if lowered.contains("uninstall") || lowered.contains("unins000") {
        return None;
    }
    Some(TargetApplication {
        display_name: display_name.trim().to_string(),
        executable_name,
        path,
        source: "installed".into(),
        last_seen_at: None,
    })
}

fn collect_uninstall_root(root: &RegKey, flags: u32, apps: &mut Vec<TargetApplication>) {
    let Ok(key) = root.open_subkey_with_flags(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        KEY_READ | flags,
    ) else {
        return;
    };
    for name in key.enum_keys().flatten() {
        let Ok(item) = key.open_subkey_with_flags(name, KEY_READ | flags) else {
            continue;
        };
        let Ok(display_name) = item.get_value::<String, _>("DisplayName") else {
            continue;
        };
        let Ok(display_icon) = item.get_value::<String, _>("DisplayIcon") else {
            continue;
        };
        if let Some(path) = executable_path(&display_icon) {
            if let Some(app) = application(display_name, path) {
                apps.push(app);
            }
        }
    }
}

fn collect_app_paths(root: &RegKey, flags: u32, apps: &mut Vec<TargetApplication>) {
    let Ok(key) = root.open_subkey_with_flags(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths",
        KEY_READ | flags,
    ) else {
        return;
    };
    for name in key.enum_keys().flatten() {
        let Ok(item) = key.open_subkey_with_flags(&name, KEY_READ | flags) else {
            continue;
        };
        let Ok(raw_path) = item.get_value::<String, _>("") else {
            continue;
        };
        if let Some(path) = executable_path(&raw_path) {
            let display_name = name.trim_end_matches(".exe").to_string();
            if let Some(app) = application(display_name, path) {
                apps.push(app);
            }
        }
    }
}

fn collect_shortcuts(apps: &mut Vec<TargetApplication>) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = r#"[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $shell=New-Object -ComObject WScript.Shell; @([Environment]::GetFolderPath('DesktopDirectory'),($env:PUBLIC+'\Desktop'),($env:APPDATA+'\Microsoft\Windows\Start Menu\Programs'),($env:ProgramData+'\Microsoft\Windows\Start Menu\Programs')) | Where-Object { $_ } | ForEach-Object { Get-ChildItem -LiteralPath $_ -Filter *.lnk -Recurse -ErrorAction SilentlyContinue } | ForEach-Object { $shortcut=$shell.CreateShortcut($_.FullName); if($shortcut.TargetPath -match '(?i)\.exe$'){ [Console]::Out.WriteLine(('{0}{1}{2}' -f $_.BaseName,[char]9,$shortcut.TargetPath)) } }"#;
    let Ok(output) = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((display_name, raw_path)) = line.split_once('\t') else {
            continue;
        };
        if let Some(path) = executable_path(raw_path) {
            if let Some(app) = application(display_name.to_string(), path) {
                apps.push(app);
            }
        }
    }
}

pub fn installed_applications() -> Vec<TargetApplication> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let local_machine = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut discovered = Vec::new();
    // Shortcut names are the labels the user sees; keep their order as priority.
    collect_shortcuts(&mut discovered);
    collect_uninstall_root(&current_user, 0, &mut discovered);
    collect_app_paths(&current_user, 0, &mut discovered);
    for flags in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        collect_uninstall_root(&local_machine, flags, &mut discovered);
        collect_app_paths(&local_machine, flags, &mut discovered);
    }

    let mut unique = BTreeMap::new();
    for app in discovered {
        unique
            .entry(app.path.to_lowercase())
            .and_modify(|current: &mut TargetApplication| {
                if current
                    .display_name
                    .eq_ignore_ascii_case(current.executable_name.trim_end_matches(".exe"))
                {
                    current.display_name = app.display_name.clone();
                }
            })
            .or_insert(app);
    }
    let mut apps = unique.into_values().collect::<Vec<_>>();
    apps.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
    });
    apps
}

pub fn application_from_path(path: &Path) -> Result<TargetApplication, String> {
    if !path.is_file()
        || !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
    {
        return Err("请选择有效的 Windows 应用程序（.exe）".into());
    }
    let display_name = super::application_identity::packaged_display_name(&path.to_string_lossy())
        .unwrap_or(
            path.file_stem()
                .and_then(|value| value.to_str())
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "无法读取应用名称".to_string())?
                .to_string(),
        );
    application(display_name, path.to_string_lossy().into_owned())
        .ok_or_else(|| "无法读取应用信息".to_string())
}

#[cfg(test)]
mod tests {
    use super::{executable_path, file_display_name};

    #[test]
    fn version_name_has_safe_fallback_and_reads_edge_when_installed() {
        assert!(file_display_name(r"C:\missing\not-an-application.exe").is_none());
        for root in [
            std::env::var("ProgramFiles(x86)"),
            std::env::var("ProgramFiles"),
        ]
        .into_iter()
        .flatten()
        {
            let edge = std::path::Path::new(&root).join(r"Microsoft\Edge\Application\msedge.exe");
            if edge.is_file() {
                assert_eq!(
                    file_display_name(&edge.to_string_lossy()).as_deref(),
                    Some("Microsoft Edge")
                );
                return;
            }
        }
    }

    #[test]
    fn rejects_non_executable_display_icons() {
        assert_eq!(executable_path("C:\\missing\\icon.dll,0"), None);
    }

    #[test]
    fn parses_quoted_executable_display_icons() {
        let current = std::env::current_exe().expect("test executable path");
        let display_icon = format!("\"{}\",0", current.display());
        assert_eq!(
            executable_path(&display_icon),
            Some(current.to_string_lossy().into_owned())
        );
    }
}
