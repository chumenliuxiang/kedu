mod app_identity;
mod bridge;
mod browser;
mod category_commands;
mod category_exchange_commands;
mod category_exchange_models;
mod category_models;
mod commands;
mod data_location;
mod domain;
mod focus_commands;
mod focus_runtime;
mod focus_statistics_commands;
mod limit_commands;
mod platform;
mod storage;
mod timeline_commands;
mod timeline_models;

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    window::Color,
    AppHandle, Emitter, Manager, Theme, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};

use storage::Storage;
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;
use tauri_plugin_notification::NotificationExt;

pub struct AppState {
    storage: Storage,
    health: Arc<Mutex<domain::RuntimeStatus>>,
    pending_restriction: Mutex<Option<(i64, domain::RestrictionNotice)>>,
}

fn system_uses_dark_theme() -> bool {
    #[cfg(windows)]
    {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
            .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme"))
            .map(|light| light == 0)
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    false
}

/// Earlier releases stored named palettes; any dark one now maps to dark mode.
fn startup_appearance(preference: &str, system_dark: bool) -> (Option<Theme>, Color) {
    let dark = match preference {
        "system" => system_dark,
        "dark" | "ink" | "midnight" | "plum" => true,
        _ => false,
    };
    // Matches --kd-bg so the window never flashes another colour while loading.
    let color = if dark {
        Color(22, 22, 24, 255)
    } else {
        Color(245, 245, 247, 255)
    };
    let native_theme = if preference == "system" {
        None
    } else if dark {
        Some(Theme::Dark)
    } else {
        Some(Theme::Light)
    };
    (native_theme, color)
}

fn create_main_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let preference = app
        .state::<AppState>()
        .storage
        .theme()
        .unwrap_or_else(|_| "light".into());
    let accent = app
        .state::<AppState>()
        .storage
        .accent()
        .unwrap_or_else(|_| "blue".into());
    let (native_theme, background) = startup_appearance(&preference, system_uses_dark_theme());
    // Seed the synchronous page initializer from the database before any UI script runs.
    let initialization = format!(
        "try {{ localStorage.setItem('kedu.theme', {}); localStorage.setItem('kedu.accent', {}); }} catch {{}}",
        serde_json::to_string(&preference).expect("theme is a string"),
        serde_json::to_string(&accent).expect("accent is a string")
    );
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("刻度")
        .inner_size(1240.0, 800.0)
        .min_inner_size(960.0, 640.0)
        .center()
        .theme(native_theme)
        .background_color(background)
        .initialization_script(initialization)
        .build()
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return;
    }
    match create_main_window(app) {
        Ok(window) => {
            let _ = window.set_focus();
        }
        Err(error) => eprintln!("unable to open main window: {error}"),
    }
}

fn start_activity_tracker(
    storage: Storage,
    app: AppHandle,
    health: Arc<Mutex<domain::RuntimeStatus>>,
    test_mode: bool,
) {
    let icon_sender = start_icon_capture(storage.clone(), app.clone());
    thread::spawn(move || {
        let mut reminded_focus = HashSet::new();
        let mut reminded_schedules = HashSet::new();
        loop {
            let _ = storage.apply_pending_rule_changes();
            if let Ok(warnings) = storage.upcoming_schedule_warnings() {
                for (schedule_id, label, remaining) in warnings {
                    let threshold = if remaining <= 60 { 60 } else { 300 };
                    if !test_mode && reminded_schedules.insert(format!("{schedule_id}-{threshold}"))
                    {
                        let _ = app
                            .notification()
                            .builder()
                            .title("专注即将开始")
                            .body(format!(
                                "“{label}”将在约 {} 分钟后开始，请准备收尾",
                                threshold / 60
                            ))
                            .show();
                    }
                }
            }
            if let Ok(Some(focus)) = storage.activate_due_schedule() {
                if !test_mode {
                    let _ = app
                        .notification()
                        .builder()
                        .title("专注已开始")
                        .body(format!("“{}”的限制已经生效", focus.label))
                        .show();
                }
                let _ = app.emit("focus-started", focus);
            }
            if let Ok(Some(focus)) = storage.active_focus().map(|focus| {
                focus.filter(|focus| focus.status == "focusing" && focus.timer_kind == "countdown")
            }) {
                let remaining = focus
                    .planned_end
                    .saturating_sub(chrono::Utc::now().timestamp_millis())
                    / 1000;
                let threshold = if (1..=60).contains(&remaining) {
                    Some(60)
                } else if (61..=300).contains(&remaining) {
                    Some(300)
                } else {
                    None
                };
                if let Some(threshold) = threshold.filter(|threshold| {
                    !test_mode && reminded_focus.insert(format!("{}-{threshold}", focus.id))
                }) {
                    let _ = app
                        .notification()
                        .builder()
                        .title("专注即将结束")
                        .body(format!("“{}”还剩约 {} 分钟", focus.label, threshold / 60))
                        .show();
                }
            }
            let idle_threshold = storage
                .settings()
                .map(|settings| settings.idle_minutes.max(1) as u64 * 60)
                .unwrap_or(300);
            let sampled_at = chrono::Utc::now().timestamp_millis();
            if let Ok(mut status) = health.lock() {
                status.last_sample_at = Some(sampled_at);
            }
            if let Some(mut activity) = platform::foreground_activity(idle_threshold) {
                // A bounded worker keeps icon I/O and window messages away from
                // collection, policy checks, and the UI thread.
                let _ = icon_sender.try_send(activity.process_path.clone());
                if activity.idle
                    && storage
                        .should_count_idle_as_active(&activity)
                        .unwrap_or(false)
                {
                    activity.idle = false;
                    activity.idle_exempt = true;
                }
                if let Err(error) = storage.record_activity(&activity) {
                    if let Ok(mut status) = health.lock() {
                        status.collector_error = Some(error.clone());
                    }
                    eprintln!("activity tracker write failed: {error}");
                } else if let Ok(mut status) = health.lock() {
                    status.last_write_at = Some(sampled_at);
                    status.collector_error = None;
                }
                if let Ok(warnings) = storage.take_limit_reminders(&activity) {
                    for warning in warnings {
                        if !test_mode {
                            let _ = app
                                .notification()
                                .builder()
                                .title("额度即将用完")
                                .sound("Default")
                                .body(format!(
                                    "“{}”还剩约 {} 分钟",
                                    warning.rule_name,
                                    (warning.remaining_seconds + 59) / 60
                                ))
                                .show();
                        }
                    }
                }
            }
            thread::sleep(Duration::from_secs(5));
        }
    });
}

fn start_icon_capture(storage: Storage, app: AppHandle) -> std::sync::mpsc::SyncSender<String> {
    use std::{collections::HashMap, time::Instant};
    let (sender, receiver) = std::sync::mpsc::sync_channel::<String>(8);
    thread::spawn(move || {
        let mut attempted = HashMap::<String, Instant>::new();
        while let Ok(path) = receiver.recv() {
            let key = path.to_lowercase();
            if attempted
                .get(&key)
                .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
            {
                continue;
            }
            if attempted.len() >= 256 {
                attempted.retain(|_, at| at.elapsed() < Duration::from_secs(60));
            }
            if attempted.len() >= 256 {
                attempted.clear();
            }
            attempted.insert(key, Instant::now());
            match storage.capture_application_icon(&path) {
                Ok(true) => {
                    let _ = app.emit("application-icons-changed", ());
                }
                Ok(false) => {}
                Err(error) => eprintln!("application icon capture failed: {error}"),
            }
        }
    });
    sender
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Some(result) = data_location::handle_cli() {
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    let database = match data_location::prepare_database() {
        Ok(database) => database,
        Err(error) => {
            let _ = rfd::MessageDialog::new()
                .set_title("刻度无法打开数据")
                .set_description(&error)
                .set_level(rfd::MessageLevel::Error)
                .show();
            eprintln!("{error}");
            return;
        }
    };
    let test_mode = data_location::test_data_dir().ok().flatten().is_some();
    let builder = tauri::Builder::default();
    let builder = if test_mode {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if !args.iter().any(|argument| argument == "--minimized") {
                show_main_window(app);
            }
        }))
    };
    let app = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(move |app| {
            let storage = Storage::new(database.clone()).map_err(std::io::Error::other)?;
            let health = Arc::new(Mutex::new(domain::RuntimeStatus {
                data_path: storage.database_path().to_string_lossy().into_owned(),
                data_earliest_at: None,
                data_latest_at: None,
                last_sample_at: None,
                last_write_at: None,
                collector_error: None,
            }));
            if data_location::test_data_dir()
                .map_err(std::io::Error::other)?
                .is_none()
            {
                let desired = storage
                    .persistent_autostart()
                    .map_err(std::io::Error::other)?
                    .unwrap_or(true);
                if desired {
                    app.autolaunch().enable()
                } else {
                    app.autolaunch().disable()
                }
                .map_err(std::io::Error::other)?;
                if storage
                    .persistent_autostart()
                    .map_err(std::io::Error::other)?
                    .is_none()
                {
                    storage
                        .set_persistent_autostart(desired)
                        .map_err(std::io::Error::other)?;
                }
            }
            app.manage(AppState {
                storage: storage.clone(),
                health: health.clone(),
                pending_restriction: Mutex::new(None),
            });
            focus_runtime::start(storage.clone(), app.handle().clone(), test_mode);
            bridge::start(storage.clone());
            start_activity_tracker(storage, app.handle().clone(), health, test_mode);

            let show = MenuItem::with_id(app, "show", "打开刻度", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::new()
                .tooltip("刻度 · 本地时间记录")
                .icon(app.default_window_icon().expect("default app icon").clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        show_main_window(tray.app_handle());
                    }
                })
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        show_main_window(app);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            if !std::env::args().any(|argument| argument == "--minimized") {
                create_main_window(app.handle())?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.destroy();
            }
        })
        .invoke_handler(tauri::generate_handler![
            timeline_commands::get_timeline_workspace,
            timeline_commands::annotate_timeline,
            timeline_commands::undo_timeline_annotation,
            commands::get_dashboard,
            commands::get_statistics,
            commands::get_runtime_status,
            commands::query_statistics,
            commands::query_activity_details,
            commands::query_website_pages,
            commands::get_website_recording_status,
            commands::query_ranking,
            commands::set_default_category,
            commands::set_activity_category,
            commands::set_statistics_excluded,
            commands::list_statistics_exclusions,
            commands::get_application_icons,
            category_commands::get_classification_workspace,
            category_commands::save_stat_category,
            category_commands::remove_stat_category,
            category_commands::assign_stat_categories,
            category_commands::create_common_stat_categories,
            category_commands::save_classification_file,
            category_exchange_commands::export_classification_package,
            category_exchange_commands::preview_classification_import,
            category_exchange_commands::apply_classification_import,
            category_exchange_commands::undo_classification_import,
            commands::get_target_catalog,
            commands::pick_application_executable,
            focus_commands::get_focus_workspace,
            focus_commands::get_focus_preferences,
            focus_commands::save_focus_preferences,
            focus_statistics_commands::query_focus_statistics,
            focus_statistics_commands::list_focus_records,
            focus_commands::get_browser_protection_status,
            focus_commands::take_restriction_notice,
            focus_commands::save_focus_task,
            focus_commands::delete_focus_task,
            focus_commands::start_focus_task,
            focus_commands::pause_focus,
            focus_commands::resume_focus,
            focus_commands::complete_focus,
            focus_commands::skip_focus_break,
            focus_commands::save_focus_whitelist,
            focus_commands::remove_focus_whitelist,
            focus_commands::save_focus_appearance,
            focus_commands::launch_focus_application,
            commands::start_focus,
            commands::start_focus_leave,
            commands::return_to_focus,
            commands::end_focus,
            commands::cancel_focus_end,
            commands::grant_exception,
            commands::list_exception_history,
            commands::end_exception,
            commands::list_rules,
            limit_commands::get_limit_reminders,
            limit_commands::save_limit_reminders,
            limit_commands::save_rule_with_reminders,
            commands::create_rule,
            commands::update_rule,
            commands::delete_rule,
            commands::set_rule_enabled,
            commands::list_classification_rules,
            commands::create_classification_rule,
            commands::update_classification_rule,
            commands::set_classification_rule_enabled,
            commands::delete_classification_rule,
            commands::get_settings,
            commands::get_theme,
            commands::set_theme,
            commands::get_accent,
            commands::set_accent,
            commands::query_insights,
            commands::update_settings,
            commands::edge_extension_path,
            commands::get_autostart,
            commands::set_autostart,
            commands::list_schedule,
            commands::import_schedule,
            commands::set_schedule_item_enabled,
            commands::set_schedule_day_enabled,
            commands::save_schedule_item,
            commands::delete_schedule_item,
            commands::export_backup,
            commands::restore_backup,
            commands::export_activity_csv,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Kedu");
    app.run(|_app, event| {
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
        }
    });
}
