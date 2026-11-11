use std::{
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::{
    domain::{FocusNotification, ScheduleNotification},
    platform, show_main_window,
    storage::Storage,
    AppState,
};

fn merge_coincident_alerts(
    schedule: &mut [ScheduleNotification],
    focus: Vec<FocusNotification>,
) -> Vec<FocusNotification> {
    focus
        .into_iter()
        .filter_map(|event| {
            if event.stage == "learning_completed" {
                if let Some(scheduled) = schedule.iter_mut().find(|s| {
                    s.stage == "schedule_end"
                        && s.session_id == Some(event.session_id)
                        && (s.occurred_at - event.occurred_at).abs() <= 1_000
                }) {
                    scheduled.notifications_enabled |= event.notifications_enabled;
                    scheduled.sound_enabled |= event.sound_enabled;
                    return None;
                }
            }
            Some(event)
        })
        .collect()
}

#[cfg(test)]
mod alert_tests {
    use super::*;
    #[test]
    fn coincident_alerts_combine_independent_sound_and_notification_channels() {
        for (schedule_notification, schedule_sound, focus_notification, focus_sound) in [
            (true, false, false, true),
            (false, true, true, false),
            (true, true, true, true),
        ] {
            let mut schedule = vec![ScheduleNotification {
                session_id: Some(3),
                stage: "schedule_end",
                label: "A".into(),
                occurred_at: 1000,
                notifications_enabled: schedule_notification,
                sound_enabled: schedule_sound,
            }];
            let focus = vec![FocusNotification {
                session_id: 3,
                stage: "learning_completed".into(),
                label: "A".into(),
                occurred_at: 1000,
                notifications_enabled: focus_notification,
                sound_enabled: focus_sound,
            }];
            assert!(merge_coincident_alerts(&mut schedule, focus).is_empty());
            assert_eq!(
                schedule[0].notifications_enabled,
                schedule_notification || focus_notification
            );
            assert_eq!(schedule[0].sound_enabled, schedule_sound || focus_sound);
        }
    }
}

/// Show once per activation/policy change. If Windows refuses focus transfer,
/// periodic policy checks must not repeatedly open or focus the main window.
#[derive(Default)]
struct GuardPresentation {
    last: Option<(isize, String, Instant)>,
    pending_activation: bool,
}
impl GuardPresentation {
    fn should_present(
        &mut self,
        window: isize,
        key: String,
        activated: bool,
        now: Instant,
    ) -> bool {
        if let Some((previous_window, previous_key, previous_at)) = &self.last {
            if *previous_window == window && previous_key == &key {
                self.pending_activation |= activated;
                if !self.pending_activation
                    || now.duration_since(*previous_at) < Duration::from_millis(750)
                {
                    return false;
                }
            }
        }
        self.pending_activation = false;
        self.last = Some((window, key, now));
        true
    }
    fn clear(&mut self) {
        self.last = None;
        self.pending_activation = false;
    }
}

#[cfg(test)]
mod guard_presentation_tests {
    use super::*;
    #[test]
    fn denied_activation_does_not_repeat_on_periodic_checks() {
        let mut gate = GuardPresentation::default();
        let now = Instant::now();
        assert!(gate.should_present(1, "focus".into(), true, now));
        assert!(!gate.should_present(1, "focus".into(), false, now + Duration::from_secs(5)));
        assert!(!gate.should_present(1, "focus".into(), false, now + Duration::from_secs(60)));
        assert!(gate.should_present(1, "focus".into(), true, now + Duration::from_secs(61)));
    }
    #[test]
    fn duplicate_events_are_throttled_without_losing_a_reactivation() {
        let mut gate = GuardPresentation::default();
        let now = Instant::now();
        assert!(gate.should_present(1, "focus".into(), true, now));
        assert!(!gate.should_present(1, "focus".into(), true, now + Duration::from_millis(100)));
        assert!(gate.should_present(1, "focus".into(), false, now + Duration::from_secs(1)));
        assert!(!gate.should_present(1, "focus".into(), false, now + Duration::from_secs(5)));
        assert!(gate.should_present(1, "quota".into(), false, now + Duration::from_secs(6)));
        gate.clear();
        assert!(gate.should_present(1, "quota".into(), false, now + Duration::from_secs(7)));
    }
}

/// The runtime advances deadlines even without a WebView. Only phase transitions
/// are emitted; the frontend renders its own display clock from backend snapshots.
pub fn start(storage: Storage, app: AppHandle, test_mode: bool) {
    // Desktop regression may opt into guarding one disposable fixture executable.
    // Every other foreground process stays untouched in isolated mode.
    let test_target = test_mode
        .then(|| {
            let directory = crate::data_location::test_data_dir().ok().flatten()?;
            let candidate = std::path::PathBuf::from(std::env::var_os("KEDU_TEST_GUARD_EXE")?);
            let candidate = candidate.canonicalize().ok()?;
            (candidate.starts_with(directory)
                && candidate.is_file()
                && candidate.extension().is_some_and(|value| value == "exe"))
            .then_some(candidate)
        })
        .flatten();
    let (sender, receiver) = mpsc::sync_channel(1);
    if !test_mode || test_target.is_some() {
        platform::watch_foreground(sender);
    }
    thread::spawn(move || {
        // Reconcile time spent asleep/exited, then discard already elapsed alerts.
        let initial = storage.latest_focus().ok().flatten();
        let _ = storage.take_focus_notifications(chrono::Utc::now().timestamp_millis());
        let _ = storage.take_schedule_notifications(chrono::Utc::now().timestamp_millis());
        let mut phase = initial.map(|session| (session.id, session.status));
        let mut last_tick = Instant::now() - Duration::from_secs(1);
        let mut last_check = Instant::now() - Duration::from_secs(5);
        let mut presentation = GuardPresentation::default();
        loop {
            let changed_window = match receiver.recv_timeout(Duration::from_secs(1)) {
                Ok(()) => true,
                Err(RecvTimeoutError::Timeout) => false,
                // In isolated mode no OS hook owns the sender.
                Err(RecvTimeoutError::Disconnected) => {
                    thread::sleep(Duration::from_secs(1));
                    false
                }
            };
            // Close short website visits promptly on a foreground change. This
            // lease check does not run the statistics aggregation or create a UI.
            if let Err(error) = storage.tick_website_recording() {
                eprintln!("website recording tick failed: {error}");
            }
            let mut phase_changed = false;
            if last_tick.elapsed() >= Duration::from_secs(1) {
                last_tick = Instant::now();
                match storage.latest_focus() {
                    Ok(current) => {
                        let next = current
                            .as_ref()
                            .map(|session| (session.id, session.status.clone()));
                        if next != phase {
                            phase = next;
                            phase_changed = true;
                            let _ = app.emit("focus-updated", current);
                        }
                    }
                    Err(error) => eprintln!("focus clock failed: {error}"),
                }
                let now = chrono::Utc::now().timestamp_millis();
                let mut schedule_events = match storage.take_schedule_notifications(now) {
                    Ok(events) => events,
                    Err(error) => {
                        eprintln!("schedule notification failed: {error}");
                        Vec::new()
                    }
                };
                let focus_events = match storage.take_focus_notifications(now) {
                    Ok(events) => merge_coincident_alerts(&mut schedule_events, events),
                    Err(error) => {
                        eprintln!("focus notification failed: {error}");
                        Vec::new()
                    }
                };
                if !test_mode {
                    for event in &schedule_events {
                        let title = if event.stage == "schedule_start" {
                            "日程开始"
                        } else {
                            "日程结束"
                        };
                        if event.notifications_enabled {
                            let mut notification =
                                app.notification().builder().title(title).body(&event.label);
                            if event.sound_enabled {
                                notification = notification.sound("Default");
                            }
                            let _ = notification.show();
                        } else if event.sound_enabled {
                            platform::play_focus_completion_sound();
                        }
                    }
                    for event in focus_events {
                        let title = if event.stage == "learning_completed" {
                            "专注完成"
                        } else {
                            "休息结束"
                        };
                        if event.notifications_enabled {
                            let mut notification =
                                app.notification().builder().title(title).body(event.label);
                            // On Windows the plugin omits the toast audio when
                            // sound is absent. Do not play a second separate sound.
                            if event.sound_enabled {
                                notification = notification.sound("Default");
                            }
                            let _ = notification.show();
                        } else if event.sound_enabled {
                            platform::play_focus_completion_sound();
                        }
                    }
                }
            }
            if (!test_mode || test_target.is_some())
                && (changed_window
                    || phase_changed
                    || last_check.elapsed() >= Duration::from_secs(5))
            {
                last_check = Instant::now();
                if let Err(error) = enforce_foreground(
                    &storage,
                    &app,
                    test_target.as_deref(),
                    &mut presentation,
                    changed_window,
                ) {
                    eprintln!("foreground guard failed: {error}");
                }
            }
        }
    });
}

fn enforce_foreground(
    storage: &Storage,
    app: &AppHandle,
    test_target: Option<&std::path::Path>,
    presentation: &mut GuardPresentation,
    activated: bool,
) -> Result<(), String> {
    let window = platform::foreground_window_id();
    if window == 0 || platform::is_essential_foreground() {
        return Ok(());
    }
    let Some(activity) = platform::foreground_activity(u64::MAX) else {
        return Ok(());
    };
    if test_target.is_some_and(|target| {
        std::path::Path::new(&activity.process_path)
            .canonicalize()
            .ok()
            .as_deref()
            != Some(target)
    }) {
        return Ok(());
    }
    if platform::foreground_window_id() != window {
        return Ok(());
    }
    // Websites are recording-only. Only application focus and daily quotas may
    // redirect to the guard page; an old browser policy cannot intervene.
    let notice = storage.restriction_for(&activity)?;
    if let Some(notice) = notice {
        let guard_mode = storage.focus_preferences()?.window_guard_mode;
        // Recheck title too: a browser can change tabs without changing HWND.
        if !platform::foreground_activity(u64::MAX).is_some_and(|current| {
            current.process_path == activity.process_path && current.title == activity.title
        }) || !platform::foreground_matches(window)
        {
            return Ok(());
        }
        let key = format!(
            "{}:{}:{:?}:{guard_mode}",
            notice.subject_kind,
            notice.subject,
            notice
                .causes
                .iter()
                .map(|cause| (cause.rule_id, cause.kind.as_str(), cause.rule_name.as_str()))
                .collect::<Vec<_>>()
        );
        if !presentation.should_present(window, key, activated, Instant::now()) {
            return Ok(());
        }
        let state = app.state::<AppState>();
        if let Ok(mut pending) = state.pending_restriction.lock() {
            *pending = Some((chrono::Utc::now().timestamp_millis(), notice.clone()));
        }
        if guard_mode == "minimize" {
            platform::minimize_restricted_foreground(window);
        }
        show_main_window(app);
        // The durable pending slot also covers creation before the JS listener loads.
        let _ = app.emit("restriction-triggered", notice);
    } else {
        presentation.clear();
    }
    Ok(())
}
