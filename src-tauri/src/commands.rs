use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::{
    domain::{
        ActivityDetails, ActivityStatistics, AppSettings, ClassificationRule, Dashboard,
        ExceptionUsage, FocusState, LimitRule, NewClassificationRule, NewLimitRule, RankingPage,
        RuleChangeResult, RuntimeStatus, ScheduleImport, ScheduleImportItem, ScheduleItem,
        StatisticsExclusion, StatisticsQuery, TargetApplication, TargetCatalog, WebsitePages,
        WebsiteRecordingStatus,
    },
    AppState,
};

// Synchronous Tauri commands run on the window thread. SQLite work belongs on
// the blocking pool so a long history query cannot stall clicks or painting.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn get_website_recording_status(
    state: State<'_, AppState>,
) -> Result<WebsiteRecordingStatus, String> {
    let storage = state.storage.clone();
    blocking(move || storage.website_recording_status()).await
}

#[tauri::command]
pub async fn query_website_pages(
    state: State<'_, AppState>,
    query: StatisticsQuery,
    site: String,
    offset: usize,
    limit: usize,
    search: Option<String>,
) -> Result<WebsitePages, String> {
    let storage = state.storage.clone();
    blocking(move || {
        storage.query_website_pages(query, &site, offset, limit, search.as_deref().unwrap_or(""))
    })
    .await
}

#[tauri::command]
pub async fn get_dashboard(
    state: State<'_, AppState>,
    include_statistics: Option<bool>,
) -> Result<Dashboard, String> {
    let storage = state.storage.clone();
    blocking(move || {
        if include_statistics.unwrap_or(true) {
            storage.dashboard()
        } else {
            storage.dashboard_with_statistics(false)
        }
    })
    .await
}

#[tauri::command]
pub async fn get_runtime_status(state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
    let mut status = state
        .health
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "无法读取采集状态".to_string())?;
    let storage = state.storage.clone();
    let (earliest, latest) = blocking(move || storage.activity_date_range()).await?;
    status.data_earliest_at = earliest;
    status.data_latest_at = latest;
    Ok(status)
}

#[tauri::command]
pub async fn get_statistics(
    state: State<'_, AppState>,
    start: i64,
    end: i64,
) -> Result<ActivityStatistics, String> {
    let storage = state.storage.clone();
    blocking(move || storage.statistics(start, end)).await
}

#[tauri::command]
pub async fn query_statistics(
    state: State<'_, AppState>,
    query: StatisticsQuery,
    include_timeline: Option<bool>,
    include_trend_series: Option<bool>,
) -> Result<ActivityStatistics, String> {
    let storage = state.storage.clone();
    blocking(move || {
        storage.query_statistics_with_options(
            query,
            include_timeline.unwrap_or(true),
            include_trend_series.unwrap_or(true),
        )
    })
    .await
}

#[tauri::command]
pub async fn query_activity_details(
    state: State<'_, AppState>,
    query: StatisticsQuery,
) -> Result<ActivityDetails, String> {
    let storage = state.storage.clone();
    blocking(move || storage.query_activity_details(query)).await
}

#[tauri::command]
pub async fn query_ranking(
    state: State<'_, AppState>,
    query: StatisticsQuery,
    kind: String,
    offset: usize,
    limit: usize,
) -> Result<RankingPage, String> {
    let storage = state.storage.clone();
    blocking(move || storage.query_ranking(query, &kind, offset, limit)).await
}

#[tauri::command]
pub async fn set_default_category(
    state: State<'_, AppState>,
    target_kind: String,
    target: String,
    category: String,
) -> Result<(), String> {
    let storage = state.storage.clone();
    blocking(move || storage.set_default_category(&target_kind, &target, &category)).await
}

#[tauri::command]
pub async fn set_activity_category(
    state: State<'_, AppState>,
    source_kind: String,
    source_id: i64,
    category: Option<String>,
    source_ids: Option<Vec<i64>>,
) -> Result<(), String> {
    let storage = state.storage.clone();
    blocking(move || {
        if let Some(ids) = source_ids {
            if !ids.contains(&source_id) {
                return Err("访问记录标识不一致".into());
            }
            storage.set_activity_categories(&source_kind, &ids, category.as_deref())
        } else {
            storage.set_activity_category(&source_kind, source_id, category.as_deref())
        }
    })
    .await
}

#[tauri::command]
pub async fn set_statistics_excluded(
    state: State<'_, AppState>,
    target_kind: String,
    target: String,
    excluded: bool,
) -> Result<(), String> {
    let storage = state.storage.clone();
    blocking(move || storage.set_statistics_excluded(&target_kind, &target, excluded)).await
}

#[tauri::command]
pub async fn list_statistics_exclusions(
    state: State<'_, AppState>,
) -> Result<Vec<StatisticsExclusion>, String> {
    let storage = state.storage.clone();
    blocking(move || storage.list_statistics_exclusions()).await
}

#[tauri::command]
pub async fn get_application_icons(
    state: State<'_, AppState>,
    targets: Vec<String>,
) -> Result<BTreeMap<String, Option<String>>, String> {
    let storage = state.storage.clone();
    let targets = targets
        .into_iter()
        .map(|target| target.trim().to_string())
        .filter(|target| !target.is_empty())
        .take(64)
        .collect::<Vec<_>>();

    tauri::async_runtime::spawn_blocking(move || {
        let mut icons = BTreeMap::new();
        for target in targets {
            if icons.contains_key(&target) {
                continue;
            }
            let icon = storage.application_icon(&target).unwrap_or_else(|error| {
                eprintln!("application icon unavailable: {error}");
                None
            });
            icons.insert(target, icon);
        }
        Ok(icons)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn get_target_catalog(state: State<'_, AppState>) -> Result<TargetCatalog, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut catalog = storage.target_catalog()?;
        let mut applications = BTreeMap::<String, TargetApplication>::new();
        let mut ambiguous = BTreeSet::new();
        for app in catalog.applications {
            let key = app.executable_name.to_lowercase();
            if let Some(existing) = applications.get(&key) {
                if !existing.path.eq_ignore_ascii_case(&app.path) {
                    ambiguous.insert(key.clone());
                }
            }
            applications.entry(key).or_insert(app);
        }
        for mut installed in crate::platform::installed_applications() {
            if is_system_explorer_path(&installed.path) {
                installed.display_name = "Windows 资源管理器".into();
            }
            let key = installed.executable_name.to_lowercase();
            if let Some(recorded) = applications.get_mut(&key) {
                let same_path = recorded
                    .path
                    .replace('/', "\\")
                    .eq_ignore_ascii_case(&installed.path.replace('/', "\\"));
                if same_path
                    && !ambiguous.contains(&key)
                    && recorded.display_name.eq_ignore_ascii_case(
                        recorded
                            .executable_name
                            .strip_suffix(".exe")
                            .or_else(|| recorded.executable_name.strip_suffix(".EXE"))
                            .unwrap_or(&recorded.executable_name),
                    )
                {
                    recorded.display_name = installed.display_name;
                }
            } else {
                applications.insert(key, installed);
            }
        }
        for (key, app) in &mut applications {
            if ambiguous.contains(key) {
                app.source = "ambiguous-history".into();
                continue;
            }
            if app.source == "history" && is_system_explorer_path(&app.path) {
                app.display_name = "Windows 资源管理器".into();
                continue;
            }
            if app.source == "history"
                && app.display_name.eq_ignore_ascii_case(
                    app.executable_name
                        .strip_suffix(".exe")
                        .or_else(|| app.executable_name.strip_suffix(".EXE"))
                        .unwrap_or(&app.executable_name),
                )
            {
                if let Some(name) = crate::platform::file_display_name(&app.path) {
                    app.display_name = name;
                }
            }
        }
        catalog.applications = canonical_catalog(applications.into_values());
        catalog.applications.sort_by(
            |left, right| match (left.last_seen_at, right.last_seen_at) {
                (Some(left), Some(right)) => right.cmp(&left),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => left
                    .display_name
                    .to_lowercase()
                    .cmp(&right.display_name.to_lowercase()),
            },
        );
        Ok(catalog)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn canonical_catalog(apps: impl IntoIterator<Item = TargetApplication>) -> Vec<TargetApplication> {
    let mut merged = BTreeMap::<String, TargetApplication>::new();
    for mut app in apps {
        let key = crate::app_identity::canonical_executable(&app.executable_name);
        if key == "steam.exe" && !app.executable_name.eq_ignore_ascii_case("steam.exe") {
            // Use the client's real launcher when only a CEF window was recorded.
            let path = Path::new(&app.path);
            if let Some(root) = path.ancestors().nth(4) {
                let helper = root.join("bin/cef/cef.win64/steamwebhelper.exe");
                if helper
                    .to_string_lossy()
                    .replace('/', "\\")
                    .eq_ignore_ascii_case(&app.path.replace('/', "\\"))
                {
                    app.path = root.join("steam.exe").to_string_lossy().into_owned();
                }
            }
        }
        app.display_name = crate::app_identity::display_name(&key, &app.display_name);
        app.executable_name = key.clone();
        if let Some(previous) = merged.get_mut(&key) {
            let seen = previous.last_seen_at.max(app.last_seen_at);
            // Prefer an available launch target, then the real client executable.
            let priority = |path: &str| {
                (
                    Path::new(path).is_file(),
                    path.replace('/', "\\")
                        .rsplit('\\')
                        .next()
                        .is_some_and(|name| name.eq_ignore_ascii_case(&key)),
                )
            };
            if priority(&app.path) > priority(&previous.path) {
                *previous = app;
            }
            previous.last_seen_at = seen;
        } else {
            merged.insert(key, app);
        }
    }
    merged.into_values().collect()
}

fn is_system_explorer_path(path: &str) -> bool {
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    is_explorer_in_windows_dir(path, &windows)
}

fn is_explorer_in_windows_dir(path: &str, windows: &str) -> bool {
    let normalized = path.replace('/', "\\");
    let expected = format!("{}\\explorer.exe", windows.trim_end_matches(['\\', '/']));
    normalized.eq_ignore_ascii_case(&expected)
}

#[cfg(test)]
mod explorer_name_tests {
    use super::is_explorer_in_windows_dir;

    #[test]
    fn only_real_windows_explorer_path_is_special() {
        assert!(is_explorer_in_windows_dir(
            "c:/WINDOWS/Explorer.EXE",
            "C:\\Windows\\"
        ));
        assert!(!is_explorer_in_windows_dir(
            "C:\\Apps\\explorer.exe",
            "C:\\Windows"
        ));
        assert!(!is_explorer_in_windows_dir(
            "C:\\Windows\\System32\\explorer.exe",
            "C:\\Windows"
        ));
    }
}

#[tauri::command]
pub fn pick_application_executable() -> Result<Option<TargetApplication>, String> {
    let Some(path) = rfd::FileDialog::new()
        .set_title("选择要限制的应用")
        .add_filter("Windows 应用程序", &["exe"])
        .pick_file()
    else {
        return Ok(None);
    };
    crate::platform::application_from_path(&path).map(|app| canonical_catalog([app]).pop())
}

#[tauri::command]
pub async fn start_focus(
    state: State<'_, AppState>,
    label: String,
    minutes: i64,
    mode: String,
) -> Result<FocusState, String> {
    let storage = state.storage.clone();
    blocking(move || storage.start_focus(label, minutes, mode)).await
}

#[tauri::command]
pub async fn start_focus_leave(state: State<'_, AppState>) -> Result<FocusState, String> {
    let storage = state.storage.clone();
    blocking(move || storage.start_focus_leave()).await
}

#[tauri::command]
pub async fn return_to_focus(state: State<'_, AppState>) -> Result<FocusState, String> {
    let storage = state.storage.clone();
    blocking(move || storage.return_to_focus()).await
}

#[tauri::command]
pub async fn end_focus(state: State<'_, AppState>) -> Result<FocusState, String> {
    let storage = state.storage.clone();
    blocking(move || storage.end_focus()).await
}

#[tauri::command]
pub async fn cancel_focus_end(state: State<'_, AppState>) -> Result<FocusState, String> {
    let storage = state.storage.clone();
    blocking(move || storage.cancel_focus_end()).await
}

#[tauri::command]
pub async fn grant_exception(
    state: State<'_, AppState>,
    kind: String,
    target: String,
    subject_kind: String,
    subject: String,
    minutes: i64,
    reason: String,
) -> Result<(), String> {
    let storage = state.storage.clone();
    blocking(move || storage.record_exception(kind, target, subject_kind, subject, minutes, reason))
        .await
}

#[tauri::command]
pub async fn list_exception_history(
    state: State<'_, AppState>,
    limit: i64,
) -> Result<Vec<ExceptionUsage>, String> {
    let storage = state.storage.clone();
    blocking(move || storage.exception_history(limit)).await
}

#[tauri::command]
pub async fn end_exception(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let storage = state.storage.clone();
    blocking(move || storage.end_exception(id)).await
}

#[tauri::command]
pub async fn list_rules(state: State<'_, AppState>) -> Result<Vec<LimitRule>, String> {
    let storage = state.storage.clone();
    blocking(move || storage.list_rules()).await
}

#[tauri::command]
pub fn create_rule(state: State<'_, AppState>, rule: NewLimitRule) -> Result<LimitRule, String> {
    state.storage.create_rule(rule)
}

#[tauri::command]
pub fn update_rule(
    state: State<'_, AppState>,
    id: i64,
    rule: NewLimitRule,
) -> Result<LimitRule, String> {
    state.storage.update_rule(id, rule)
}

#[tauri::command]
pub fn delete_rule(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.storage.delete_rule(id)
}

#[tauri::command]
pub fn set_rule_enabled(
    state: State<'_, AppState>,
    id: i64,
    enabled: bool,
) -> Result<RuleChangeResult, String> {
    state.storage.set_rule_enabled(id, enabled)
}

#[tauri::command]
pub async fn list_classification_rules(
    state: State<'_, AppState>,
) -> Result<Vec<ClassificationRule>, String> {
    let storage = state.storage.clone();
    blocking(move || storage.list_classification_rules()).await
}

#[tauri::command]
pub fn create_classification_rule(
    state: State<'_, AppState>,
    rule: NewClassificationRule,
) -> Result<ClassificationRule, String> {
    state.storage.create_classification_rule(rule)
}

#[tauri::command]
pub fn update_classification_rule(
    state: State<'_, AppState>,
    id: i64,
    rule: NewClassificationRule,
) -> Result<ClassificationRule, String> {
    state.storage.update_classification_rule(id, rule)
}

#[tauri::command]
pub fn set_classification_rule_enabled(
    state: State<'_, AppState>,
    id: i64,
    enabled: bool,
) -> Result<(), String> {
    state.storage.set_classification_rule_enabled(id, enabled)
}

#[tauri::command]
pub fn delete_classification_rule(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.storage.delete_classification_rule(id)
}

#[tauri::command]
pub async fn get_theme(state: State<'_, AppState>) -> Result<String, String> {
    let storage = state.storage.clone();
    blocking(move || storage.theme()).await
}

#[tauri::command]
pub async fn set_theme(state: State<'_, AppState>, theme: String) -> Result<String, String> {
    let storage = state.storage.clone();
    blocking(move || storage.set_theme(&theme)).await
}

#[tauri::command]
pub async fn get_accent(state: State<'_, AppState>) -> Result<String, String> {
    let storage = state.storage.clone();
    blocking(move || storage.accent()).await
}

#[tauri::command]
pub async fn set_accent(state: State<'_, AppState>, accent: String) -> Result<String, String> {
    let storage = state.storage.clone();
    blocking(move || storage.set_accent(&accent)).await
}

#[tauri::command]
pub async fn query_insights(
    state: State<'_, AppState>,
    start: i64,
    end: i64,
) -> Result<crate::storage::UsageInsights, String> {
    let storage = state.storage.clone();
    blocking(move || storage.query_insights(start, end, chrono::Utc::now().timestamp_millis()))
        .await
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    let storage = state.storage.clone();
    blocking(move || storage.settings()).await
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    state.storage.update_settings(settings)
}

#[tauri::command]
pub fn edge_extension_path(app: AppHandle) -> Result<String, String> {
    let bundled = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?
        .join("edge-extension");
    let path = if bundled.exists() {
        bundled
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join("edge-extension")
    };
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_autostart(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<bool, String> {
    if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(|error| error.to_string())?;
    state.storage.set_persistent_autostart(enabled)?;
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_schedule(
    state: State<'_, AppState>,
    date: String,
) -> Result<Vec<ScheduleItem>, String> {
    let storage = state.storage.clone();
    blocking(move || storage.list_schedule(date)).await
}

#[tauri::command]
pub async fn import_schedule(
    state: State<'_, AppState>,
    date: String,
    schedule: ScheduleImport,
) -> Result<Vec<ScheduleItem>, String> {
    let storage = state.storage.clone();
    blocking(move || storage.import_schedule(date, schedule)).await
}

#[tauri::command]
pub async fn set_schedule_item_enabled(
    state: State<'_, AppState>,
    id: i64,
    enabled: bool,
) -> Result<(), String> {
    let storage = state.storage.clone();
    blocking(move || storage.set_schedule_item_enabled(id, enabled)).await
}

#[tauri::command]
pub async fn set_schedule_day_enabled(
    state: State<'_, AppState>,
    date: String,
    enabled: bool,
) -> Result<(), String> {
    let storage = state.storage.clone();
    blocking(move || storage.set_schedule_day_enabled(date, enabled)).await
}

#[tauri::command]
pub async fn save_schedule_item(
    state: State<'_, AppState>,
    id: Option<i64>,
    date: String,
    item: ScheduleImportItem,
) -> Result<ScheduleItem, String> {
    let storage = state.storage.clone();
    blocking(move || storage.save_schedule_item(id, date, item)).await
}

#[tauri::command]
pub async fn delete_schedule_item(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let storage = state.storage.clone();
    blocking(move || storage.delete_schedule_item(id)).await
}

#[tauri::command]
pub fn export_backup(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let name = format!(
        "刻度备份-{}.kedu.sqlite",
        chrono::Local::now().format("%Y%m%d-%H%M")
    );
    let Some(path) = rfd::FileDialog::new()
        .set_title("导出刻度备份")
        .set_file_name(name)
        .add_filter("刻度备份", &["sqlite"])
        .save_file()
    else {
        return Ok(None);
    };
    state.storage.backup_to(path.clone())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn restore_backup(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let Some(path) = rfd::FileDialog::new()
        .set_title("恢复刻度备份")
        .add_filter("刻度备份", &["sqlite"])
        .pick_file()
    else {
        return Ok(None);
    };
    state.storage.restore_from(path.clone())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn export_activity_csv(
    state: State<'_, AppState>,
    start: i64,
    end: i64,
) -> Result<Option<String>, String> {
    let name = format!(
        "刻度活动-{}.csv",
        chrono::Local::now().format("%Y%m%d-%H%M")
    );
    let Some(path) = rfd::FileDialog::new()
        .set_title("导出活动明细")
        .set_file_name(name)
        .add_filter("CSV 表格", &["csv"])
        .save_file()
    else {
        return Ok(None);
    };
    state.storage.export_csv(path.clone(), start, end)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}
