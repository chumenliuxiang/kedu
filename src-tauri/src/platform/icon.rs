use std::{
    collections::HashMap,
    ffi::OsStr,
    io::Cursor,
    os::windows::ffi::OsStrExt,
    ptr::null_mut,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{imageops::FilterType, ImageFormat, RgbaImage};
use windows_sys::Win32::{
    Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC,
        BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
    },
    UI::{
        Shell::ExtractIconExW,
        WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO},
    },
};

const MAX_CACHE_ENTRIES: usize = 128;
const POSITIVE_TTL: Duration = Duration::from_secs(10 * 60);
const NEGATIVE_TTL: Duration = Duration::from_secs(60);
const MAX_ICON_EDGE: u32 = 64;

#[derive(Clone)]
struct CacheEntry {
    inserted_at: Instant,
    value: Option<String>,
}

struct OwnedIcon(HICON);

impl OwnedIcon {
    fn new(handle: HICON) -> Option<Self> {
        (!handle.is_null()).then_some(Self(handle))
    }
}

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        unsafe {
            DestroyIcon(self.0);
        }
    }
}

struct OwnedBitmap(HBITMAP);

impl OwnedBitmap {
    fn new(handle: HBITMAP) -> Option<Self> {
        (!handle.is_null()).then_some(Self(handle))
    }
}

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}

struct ScreenDc(HDC);

impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(null_mut(), self.0);
        }
    }
}

struct MemoryDc(HDC);

impl Drop for MemoryDc {
    fn drop(&mut self) {
        unsafe {
            DeleteDC(self.0);
        }
    }
}

pub fn application_icon_data_url(path: &str) -> Option<String> {
    let key = path.trim().to_lowercase();
    if key.is_empty() {
        return None;
    }
    if let Some(cached) = read_cache(&key) {
        return cached;
    }
    let value = steam_owner_path(path)
        .and_then(|owner| extract_icon(&owner))
        .or_else(|| packaged_icon(path))
        .or_else(|| extract_icon(path.trim()))
        .or_else(|| foreground_application_icon(path))
        .or_else(|| shortcut_icon(path));
    write_cache(key, value.clone());
    value
}

fn steam_owner_path(path: &str) -> Option<String> {
    let executable = std::path::Path::new(path);
    if !executable
        .file_name()?
        .eq_ignore_ascii_case("steamwebhelper.exe")
    {
        return None;
    }
    // Known Steam CEF layout only; never map arbitrary games to their launcher.
    let root = executable.parent()?.parent()?.parent()?.parent()?;
    let helper = root.join("bin/cef/cef.win64/steamwebhelper.exe");
    let owner = root.join("steam.exe");
    (helper
        .to_string_lossy()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&path.replace('/', "\\"))
        && owner.is_file())
    .then(|| owner.to_string_lossy().into_owned())
}

pub fn foreground_application_icon(path: &str) -> Option<String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassLongPtrW, GetForegroundWindow, SendMessageTimeoutW, GCLP_HICON, GCLP_HICONSM,
        ICON_BIG, ICON_SMALL2, SMTO_ABORTIFHUNG, WM_GETICON,
    };
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null()
            || !super::application_identity::window_process_path(window)?.eq_ignore_ascii_case(path)
        {
            return None;
        }
        for size in [ICON_BIG, ICON_SMALL2] {
            let mut result = 0usize;
            SendMessageTimeoutW(
                window,
                WM_GETICON,
                size as usize,
                0,
                SMTO_ABORTIFHUNG,
                100,
                &mut result,
            );
            if result != 0 {
                if let Some(image) = icon_to_data_url(result as HICON) {
                    return Some(image);
                }
            }
        }
        for field in [GCLP_HICON, GCLP_HICONSM] {
            let icon = GetClassLongPtrW(window, field) as HICON;
            if !icon.is_null() {
                if let Some(image) = icon_to_data_url(icon) {
                    return Some(image);
                }
            }
        }
    }
    None
}

fn packaged_icon(path: &str) -> Option<String> {
    use quick_xml::{events::Event, Reader};
    let exe = std::path::Path::new(path);
    if !exe.components().any(|p| {
        p.as_os_str().eq_ignore_ascii_case("WindowsApps")
            || p.as_os_str().eq_ignore_ascii_case("SystemApps")
    }) {
        return None;
    }
    for folder in exe.ancestors().skip(1).take(6) {
        let manifest = folder.join("AppxManifest.xml");
        if !manifest.is_file() {
            continue;
        }
        if std::fs::metadata(&manifest).ok()?.len() > 1_048_576 {
            return None;
        }
        let xml = std::fs::read_to_string(manifest).ok()?;
        let mut reader = Reader::from_str(&xml);
        let relative = exe
            .strip_prefix(folder)
            .ok()?
            .to_string_lossy()
            .replace('/', "\\");
        let mut matched = false;
        let mut assets = Vec::new();
        loop {
            match reader.read_event().ok()? {
                Event::Start(e) | Event::Empty(e) => {
                    if e.local_name().as_ref() == b"Application" {
                        matched = e.attributes().flatten().any(|a| {
                            a.key.as_ref() == b"Executable"
                                && String::from_utf8_lossy(&a.value)
                                    .replace('/', "\\")
                                    .eq_ignore_ascii_case(&relative)
                        });
                    } else if matched && e.local_name().as_ref() == b"VisualElements" {
                        for a in e.attributes().flatten() {
                            if matches!(a.key.as_ref(), b"Square44x44Logo" | b"Square150x150Logo") {
                                assets.push(String::from_utf8_lossy(&a.value).into_owned());
                            }
                        }
                    }
                }
                Event::End(e) if e.local_name().as_ref() == b"Application" => matched = false,
                Event::Eof => break,
                _ => {}
            }
        }
        for asset in assets {
            let base = folder.join(asset.replace('\\', "/"));
            if !base.starts_with(folder) {
                continue;
            }
            let mut candidates = vec![base.clone()];
            if let (Some(parent), Some(stem)) = (base.parent(), base.file_stem()) {
                if let Ok(entries) = std::fs::read_dir(parent) {
                    for entry in entries.flatten().take(512) {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if name.starts_with(&format!("{}.", stem.to_string_lossy()))
                            && name.ends_with(".png")
                        {
                            candidates.push(entry.path());
                        }
                    }
                }
            }
            for file in candidates {
                if let Ok(meta) = std::fs::metadata(&file) {
                    if meta.len() <= 2_097_152 {
                        if let Ok(image) = image::open(&file) {
                            let mut output = Cursor::new(Vec::new());
                            image
                                .resize(64, 64, FilterType::Lanczos3)
                                .write_to(&mut output, ImageFormat::Png)
                                .ok()?;
                            return Some(format!(
                                "data:image/png;base64,{}",
                                STANDARD.encode(output.into_inner())
                            ));
                        }
                    }
                }
            }
        }
    }
    None
}

fn shortcut_icon(path: &str) -> Option<String> {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    type Shortcuts = (Instant, Vec<(String, String)>);
    static SHORTCUTS: OnceLock<Mutex<Option<Shortcuts>>> = OnceLock::new();
    let mut cache = SHORTCUTS.get_or_init(Default::default).lock().ok()?;
    if cache
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(600))
    {
        let script = r#"[Console]::OutputEncoding=[Text.UTF8Encoding]::new();$w=New-Object -ComObject WScript.Shell;@([Environment]::GetFolderPath('DesktopDirectory'),($env:PUBLIC+'\Desktop'),($env:APPDATA+'\Microsoft\Windows\Start Menu\Programs'),($env:ProgramData+'\Microsoft\Windows\Start Menu\Programs'))|ForEach-Object{Get-ChildItem -LiteralPath $_ -Filter *.lnk -Recurse -ErrorAction SilentlyContinue}|Select-Object -First 1000|ForEach-Object{$s=$w.CreateShortcut($_.FullName);if($s.TargetPath -and $s.IconLocation){[Console]::WriteLine($s.TargetPath+[char]9+$s.IconLocation)}}"#;
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .creation_flags(0x08000000)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        // Drain stdout concurrently, so a large shortcut list cannot fill a pipe.
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().ok()?.is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let bytes = reader.join().ok()?;
        let values = String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| line.split_once('\t').map(|(a, b)| (a.into(), b.into())))
            .collect();
        *cache = Some((Instant::now(), values));
    }
    let source = cache
        .as_ref()?
        .1
        .iter()
        .find(|(target, _)| target.eq_ignore_ascii_case(path))?
        .1
        .clone();
    drop(cache);
    let (file, index) = source
        .rsplit_once(',')
        .map(|(p, i)| {
            (
                p.trim().trim_matches('"'),
                i.trim().parse::<i32>().unwrap_or(0),
            )
        })
        .unwrap_or((source.as_str(), 0));
    extract_icon_at(file, index)
}

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fresh(entry: &CacheEntry) -> bool {
    let ttl = if entry.value.is_some() {
        POSITIVE_TTL
    } else {
        NEGATIVE_TTL
    };
    entry.inserted_at.elapsed() <= ttl
}

fn read_cache(key: &str) -> Option<Option<String>> {
    let mut cache = cache().lock().ok()?;
    let entry = cache.get(key)?;
    if fresh(entry) {
        Some(entry.value.clone())
    } else {
        cache.remove(key);
        None
    }
}

fn write_cache(key: String, value: Option<String>) {
    let Ok(mut cache) = cache().lock() else {
        return;
    };
    cache.retain(|_, entry| fresh(entry));
    if cache.len() >= MAX_CACHE_ENTRIES && !cache.contains_key(&key) {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.inserted_at)
            .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(
        key,
        CacheEntry {
            inserted_at: Instant::now(),
            value,
        },
    );
}

fn extract_icon(path: &str) -> Option<String> {
    extract_icon_at(path, 0)
}

fn extract_icon_at(path: &str, index: i32) -> Option<String> {
    let wide: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut large = null_mut();
        let mut small = null_mut();
        let extracted = ExtractIconExW(wide.as_ptr(), index, &mut large, &mut small, 1);
        if extracted == 0 || extracted == u32::MAX {
            return None;
        }

        let large = OwnedIcon::new(large);
        let small = OwnedIcon::new(small);
        let handle = large
            .as_ref()
            .map(|icon| icon.0)
            .or_else(|| small.as_ref().map(|icon| icon.0))?;
        icon_to_data_url(handle)
    }
}

unsafe fn icon_to_data_url(icon: HICON) -> Option<String> {
    let mut info = ICONINFO::default();
    if GetIconInfo(icon, &mut info) == 0 {
        return None;
    }
    let _mask = OwnedBitmap::new(info.hbmMask);
    let color = OwnedBitmap::new(info.hbmColor)?;

    let mut bitmap = BITMAP::default();
    if GetObjectW(
        color.0,
        std::mem::size_of::<BITMAP>() as i32,
        &mut bitmap as *mut _ as *mut _,
    ) == 0
    {
        return None;
    }
    let width = bitmap.bmWidth.unsigned_abs();
    let height = bitmap.bmHeight.unsigned_abs();
    if width == 0 || height == 0 {
        return None;
    }

    let screen_dc = ScreenDc(GetDC(null_mut()));
    if screen_dc.0.is_null() {
        return None;
    }
    let memory_dc = MemoryDc(CreateCompatibleDC(screen_dc.0));
    if memory_dc.0.is_null() {
        return None;
    }

    let mut bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..BITMAPINFOHEADER::default()
        },
        ..BITMAPINFO::default()
    };
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    if GetDIBits(
        memory_dc.0,
        color.0,
        0,
        height,
        pixels.as_mut_ptr() as *mut _,
        &mut bitmap_info,
        DIB_RGB_COLORS,
    ) == 0
    {
        return None;
    }

    let missing_alpha = pixels.chunks_exact(4).all(|pixel| pixel[3] == 0);
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        if missing_alpha {
            pixel[3] = 255;
        }
    }

    let source = RgbaImage::from_raw(width, height, pixels)?;
    let image = if width > MAX_ICON_EDGE || height > MAX_ICON_EDGE {
        image::imageops::resize(
            &source,
            width.min(MAX_ICON_EDGE),
            height.min(MAX_ICON_EDGE),
            FilterType::Lanczos3,
        )
    } else {
        source
    };
    let mut encoded = Cursor::new(Vec::new());
    image.write_to(&mut encoded, ImageFormat::Png).ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        STANDARD.encode(encoded.into_inner())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_icon_target_uses_fallback() {
        assert_eq!(application_icon_data_url("  "), None);
    }

    #[test]
    fn missing_icon_is_negative_cached() {
        let missing = format!("C:\\missing-kedu-icon-{}.exe", std::process::id());
        assert_eq!(application_icon_data_url(&missing), None);
        assert!(read_cache(&missing.to_lowercase()).is_some());
    }

    #[test]
    fn extracts_a_windows_application_icon() {
        let windows = std::env::var("WINDIR").expect("WINDIR is set on Windows");
        let notepad = std::path::Path::new(&windows)
            .join("System32")
            .join("notepad.exe");
        let icon = application_icon_data_url(&notepad.to_string_lossy())
            .expect("notepad should expose an application icon");
        assert!(icon.starts_with("data:image/png;base64,"));
    }
}
