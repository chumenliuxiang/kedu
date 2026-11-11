use tauri::State;

use crate::{
    domain::{
        FocusAppearance, FocusPreferences, FocusState, FocusTask, FocusTaskInput,
        FocusWhitelistEntry, FocusWhitelistInput, FocusWorkspace,
    },
    AppState,
};

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn get_browser_protection_status(
) -> Result<crate::browser::BrowserProtectionStatus, String> {
    blocking(|| Ok(crate::browser::protection_status())).await
}

#[tauri::command]
pub fn take_restriction_notice(
    state: State<'_, AppState>,
) -> Option<crate::domain::RestrictionNotice> {
    state
        .pending_restriction
        .lock()
        .ok()?
        .take()
        .filter(|(at, _)| (0..=15_000).contains(&(chrono::Utc::now().timestamp_millis() - at)))
        .map(|(_, notice)| notice)
}

#[tauri::command]
pub async fn get_focus_workspace(state: State<'_, AppState>) -> Result<FocusWorkspace, String> {
    let storage = state.storage.clone();
    blocking(move || storage.focus_workspace()).await
}

#[tauri::command]
pub async fn get_focus_preferences(state: State<'_, AppState>) -> Result<FocusPreferences, String> {
    let storage = state.storage.clone();
    blocking(move || storage.focus_preferences()).await
}

#[tauri::command]
pub async fn save_focus_preferences(
    state: State<'_, AppState>,
    preferences: FocusPreferences,
) -> Result<FocusPreferences, String> {
    let storage = state.storage.clone();
    blocking(move || storage.save_focus_preferences(preferences)).await
}

#[tauri::command]
pub async fn save_focus_task(
    state: State<'_, AppState>,
    input: FocusTaskInput,
) -> Result<FocusTask, String> {
    let storage = state.storage.clone();
    blocking(move || storage.save_focus_task(input)).await
}

#[tauri::command]
pub async fn delete_focus_task(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let storage = state.storage.clone();
    blocking(move || storage.delete_focus_task(id)).await
}

#[tauri::command]
pub async fn start_focus_task(
    state: State<'_, AppState>,
    task_id: i64,
) -> Result<FocusState, String> {
    let storage = state.storage.clone();
    blocking(move || storage.start_focus_task(task_id)).await
}

#[tauri::command]
pub async fn pause_focus(
    state: State<'_, AppState>,
    session_id: i64,
) -> Result<FocusState, String> {
    let storage = state.storage.clone();
    blocking(move || storage.pause_focus(session_id)).await
}

#[tauri::command]
pub async fn resume_focus(
    state: State<'_, AppState>,
    session_id: i64,
) -> Result<FocusState, String> {
    let storage = state.storage.clone();
    blocking(move || storage.resume_focus(session_id)).await
}

#[tauri::command]
pub async fn complete_focus(
    state: State<'_, AppState>,
    session_id: i64,
) -> Result<FocusState, String> {
    let storage = state.storage.clone();
    blocking(move || storage.complete_focus(session_id)).await
}

#[tauri::command]
pub async fn skip_focus_break(
    state: State<'_, AppState>,
    session_id: i64,
) -> Result<FocusState, String> {
    let storage = state.storage.clone();
    blocking(move || storage.skip_focus_break(session_id)).await
}

#[tauri::command]
pub async fn save_focus_whitelist(
    state: State<'_, AppState>,
    input: FocusWhitelistInput,
) -> Result<FocusWhitelistEntry, String> {
    let storage = state.storage.clone();
    blocking(move || storage.save_focus_whitelist(input)).await
}

#[tauri::command]
pub async fn remove_focus_whitelist(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let storage = state.storage.clone();
    blocking(move || storage.remove_focus_whitelist(id)).await
}

#[tauri::command]
pub async fn save_focus_appearance(
    state: State<'_, AppState>,
    appearance: FocusAppearance,
) -> Result<FocusAppearance, String> {
    let storage = state.storage.clone();
    blocking(move || storage.save_focus_appearance(appearance)).await
}

#[tauri::command]
pub async fn launch_focus_application(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let storage = state.storage.clone();
    blocking(move || {
        let entry = storage
            .focus_workspace()?
            .whitelist
            .into_iter()
            .find(|item| item.id == id && item.target_kind == "application")
            .ok_or("未找到白名单应用")?;
        if entry.available_at > chrono::Utc::now().timestamp_millis() {
            return Err("该应用将在等待结束后加入白名单".into());
        }
        let target = crate::app_identity::canonical_executable(&entry.target);
        let launch_path = |path: String| {
            if target == "steam.exe" {
                let executable = std::path::Path::new(&path);
                if let Some(root) = executable.ancestors().nth(4) {
                    let helper = root.join("bin/cef/cef.win64/steamwebhelper.exe");
                    if helper
                        .to_string_lossy()
                        .replace('/', "\\")
                        .eq_ignore_ascii_case(&path.replace('/', "\\"))
                    {
                        return root.join("steam.exe").to_string_lossy().into_owned();
                    }
                }
            }
            path
        };
        let valid_path = |path: &str| {
            let path = std::path::Path::new(path);
            path.is_file()
                && path.extension().is_some_and(|extension| {
                    extension.to_string_lossy().eq_ignore_ascii_case("exe")
                })
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(&target))
        };
        let mut path = entry.path.map(launch_path).filter(|path| valid_path(path));
        if path.is_none() {
            path = storage
                .latest_application_path(&entry.target)?
                .map(launch_path)
                .filter(|path| valid_path(path));
        }
        if path.is_none() {
            path = crate::platform::installed_applications()
                .into_iter()
                .find(|app| {
                    app.executable_name.eq_ignore_ascii_case(&target) && valid_path(&app.path)
                })
                .map(|app| app.path);
        }
        let path = path.ok_or("应用路径已失效，请重新选择该应用")?;
        // Only a validated catalog EXE is executed. No shell or user-supplied arguments.
        std::process::Command::new(&path)
            .current_dir(std::path::Path::new(&path).parent().ok_or("应用路径无效")?)
            .spawn()
            .map_err(|error| format!("无法打开应用：{error}"))?;
        Ok(())
    })
    .await
}
