//! Persistent focus state. All state transitions use one IMMEDIATE transaction;
//! a render or collector poll may observe time but cannot consume a pause twice.
use super::*;
use crate::domain::{
    FocusAppearance, FocusNotification, FocusPreferences, FocusTask, FocusTaskInput,
    FocusWhitelistEntry, FocusWhitelistInput, FocusWorkspace, ScheduleNotification,
};
use rusqlite::{Row, Transaction, TransactionBehavior};

const SELECT_SESSION: &str = "SELECT id,label,mode,started_at,task_id,timer_kind,duration_ms,
    elapsed_ms,phase,phase_started_at,pause_used_ms,completed_at,completion_reason,break_ends_at,
    leave_budget_seconds,rest_duration_ms
    FROM focus_sessions";

pub(super) fn migrate(tx: &Transaction<'_>, old_version: i64) -> Result<(), String> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS focus_tasks (
           id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL,
           timer_kind TEXT NOT NULL CHECK(timer_kind IN ('countdown','countup')),
           duration_minutes INTEGER NOT NULL, mode TEXT NOT NULL,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, deleted_at INTEGER);
         CREATE TABLE IF NOT EXISTS focus_whitelist (
           id INTEGER PRIMARY KEY AUTOINCREMENT, target_kind TEXT NOT NULL,
           target TEXT NOT NULL, display_name TEXT NOT NULL, path TEXT,
           available_at INTEGER NOT NULL, UNIQUE(target_kind,target));
         CREATE TABLE IF NOT EXISTS focus_pauses (
           id INTEGER PRIMARY KEY AUTOINCREMENT, session_id INTEGER NOT NULL,
           started_at INTEGER NOT NULL, ended_at INTEGER, UNIQUE(session_id,started_at));
         CREATE TABLE IF NOT EXISTS focus_events (
           session_id INTEGER NOT NULL, stage TEXT NOT NULL, occurred_at INTEGER NOT NULL,
           consumed INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(session_id,stage));
         CREATE TABLE IF NOT EXISTS schedule_alert_events (
           schedule_id INTEGER NOT NULL, stage TEXT NOT NULL, occurred_at INTEGER NOT NULL,
           PRIMARY KEY(schedule_id,stage,occurred_at));",
    )
    .map_err(|e| e.to_string())?;
    if old_version < 7 {
        // Seed once, so removing this default later remains a deliberate choice.
        // Keep any existing Explorer entry's identity, availability and path.
        let windows_dir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let explorer_path = format!(
            "{}\\explorer.exe",
            windows_dir.trim_end_matches(['\\', '/'])
        );
        tx.execute(
            "INSERT INTO focus_whitelist(target_kind,target,display_name,path,available_at)
             SELECT 'application','explorer.exe','Windows 资源管理器',?1,0
             WHERE NOT EXISTS (SELECT 1 FROM focus_whitelist
                 WHERE target_kind='application' AND lower(target)='explorer.exe')",
            params![explorer_path],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE focus_whitelist SET display_name='Windows 资源管理器'
             WHERE target_kind='application' AND lower(target)='explorer.exe'",
            [],
        )
        .map_err(|e| e.to_string())?;
    }
    for (table, column, declaration) in [
        ("focus_sessions", "task_id", "INTEGER"),
        (
            "focus_sessions",
            "timer_kind",
            "TEXT NOT NULL DEFAULT 'countdown'",
        ),
        (
            "focus_sessions",
            "duration_ms",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("focus_sessions", "elapsed_ms", "INTEGER NOT NULL DEFAULT 0"),
        (
            "focus_sessions",
            "phase",
            "TEXT NOT NULL DEFAULT 'completed'",
        ),
        (
            "focus_sessions",
            "phase_started_at",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "focus_sessions",
            "pause_used_ms",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("focus_sessions", "completed_at", "INTEGER"),
        ("focus_sessions", "completion_reason", "TEXT"),
        ("focus_sessions", "break_ends_at", "INTEGER"),
        (
            "focus_sessions",
            "rest_duration_ms",
            "INTEGER NOT NULL DEFAULT 300000",
        ),
        ("scheduled_focus", "task_id", "INTEGER"),
    ] {
        let mut stmt = tx
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|e| e.to_string())?;
        let exists = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .iter()
            .any(|c| c == column);
        drop(stmt);
        if !exists {
            tx.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
            ))
            .map_err(|e| e.to_string())?;
        }
    }
    if old_version < 3 {
        // Do not infer historical task ownership, pause intervals or completion counts.
        // Existing scalar fields and IDs are retained unchanged.
        let now = Utc::now().timestamp_millis();
        tx.execute(
            "UPDATE focus_sessions SET duration_ms=MAX(0,planned_end-started_at),
             phase=CASE WHEN ended_at IS NOT NULL OR (planned_end<=?1 AND active_leave_started_at IS NULL) THEN 'completed'
                        WHEN active_leave_started_at IS NOT NULL THEN 'away' ELSE 'focusing' END,
             pause_used_ms=MIN(300000,MAX(0,leave_used_seconds*1000)),
             elapsed_ms=CASE WHEN ended_at IS NOT NULL THEN 0 ELSE
               MIN(MAX(0,planned_end-started_at),MAX(0,COALESCE(active_leave_started_at,?1)-started_at-leave_used_seconds*1000)) END,
             phase_started_at=COALESCE(active_leave_started_at,?1)",
            params![now],
        ).map_err(|e| e.to_string())?;
        // A legacy race could leave multiple unfinished rows. Retain them as history,
        // while only the latest session can remain live.
        tx.execute(
            "UPDATE focus_sessions SET phase='completed' WHERE phase IN ('focusing','away','resting')
             AND id != (SELECT id FROM focus_sessions WHERE phase IN ('focusing','away','resting')
                        ORDER BY started_at DESC,id DESC LIMIT 1)", [],
        ).map_err(|e| e.to_string())?;
    }
    tx.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_focus_one_live ON focus_sessions((1))
          WHERE phase IN ('focusing','away','resting');
         CREATE INDEX IF NOT EXISTS idx_focus_task_completion ON focus_sessions(task_id,completed_at);
         CREATE INDEX IF NOT EXISTS idx_focus_completed_time ON focus_sessions(completed_at DESC,id DESC)
           WHERE completed_at IS NOT NULL;"
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> Storage {
        Storage::new(std::env::temp_dir().join(format!(
            "kedu-focus-{name}-{}-{}.sqlite",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        )))
        .unwrap()
    }

    fn task(storage: &Storage, kind: &str, minutes: i64, mode: &str) -> FocusTask {
        storage
            .save_focus_task(FocusTaskInput {
                id: None,
                name: "学习".into(),
                timer_kind: kind.into(),
                duration_minutes: minutes,
                mode: mode.into(),
            })
            .unwrap()
    }

    fn whitelist(storage: &Storage, kind: &str, target: &str) -> FocusWhitelistEntry {
        storage
            .save_focus_whitelist(FocusWhitelistInput {
                id: None,
                target_kind: kind.into(),
                target: target.into(),
                display_name: target.into(),
                path: None,
            })
            .unwrap()
    }

    fn activity(name: &str) -> ActivityIdentity {
        ActivityIdentity {
            app_name: name.into(),
            process_path: format!("C:\\Apps\\{name}.exe"),
            title: "测试".into(),
            idle: false,
            idle_seconds: 0,
            idle_exempt: false,
        }
    }

    #[test]
    fn fractional_pauses_accumulate_and_freeze_both_timer_kinds() {
        for kind in ["countdown", "countup"] {
            let storage = store(kind);
            let task = task(&storage, kind, 25, "normal");
            let t = 1_000_000;
            let started = storage.start_task_at(task.id, t).unwrap();
            let paused = storage
                .focus_action_at(started.id, "pause", t + 40_321)
                .unwrap();
            assert_eq!(paused.elapsed_milliseconds, 40_321);
            assert_eq!(
                storage
                    .latest_focus_at(t + 50_321)
                    .unwrap()
                    .unwrap()
                    .elapsed_milliseconds,
                40_321
            );
            let resumed = storage
                .focus_action_at(started.id, "resume", t + 50_999)
                .unwrap();
            assert_eq!(resumed.leave_remaining_milliseconds, 289_322);
            storage
                .focus_action_at(started.id, "pause", t + 60_123)
                .unwrap();
            let resumed = storage
                .focus_action_at(started.id, "resume", t + 60_456)
                .unwrap();
            assert_eq!(resumed.leave_remaining_milliseconds, 288_989);
            assert_eq!(resumed.elapsed_milliseconds, 49_445);
            let same = storage
                .focus_action_at(started.id, "resume", t + 60_456)
                .unwrap();
            assert_eq!(
                same.leave_remaining_milliseconds,
                resumed.leave_remaining_milliseconds
            );
            let complete = storage
                .focus_action_at(started.id, "complete", t + 61_000)
                .unwrap();
            assert_eq!(complete.elapsed_milliseconds, 49_989);
            assert_eq!(complete.status, "resting");
        }
    }

    #[test]
    fn pause_exhaustion_resumes_at_deadline_even_after_process_exit() {
        let storage = store("offline-pause");
        let task = task(&storage, "countdown", 1, "normal");
        let t = 1_000_000;
        let session = storage.start_task_at(task.id, t).unwrap();
        storage
            .focus_action_at(session.id, "pause", t + 10_000)
            .unwrap();
        drop(storage.clone());
        let reopened = Storage::new(storage.path.clone()).unwrap();
        let resumed = reopened.latest_focus_at(t + 315_000).unwrap().unwrap();
        assert_eq!(resumed.status, "focusing");
        assert_eq!(resumed.elapsed_milliseconds, 15_000);
        assert_eq!(resumed.leave_remaining_milliseconds, 0);
        assert!(reopened
            .focus_action_at(session.id, "pause", t + 315_000)
            .is_err());
        let done = reopened.latest_focus_at(t + 1_000_000).unwrap().unwrap();
        assert_eq!(done.status, "completed");
        assert_eq!(done.elapsed_milliseconds, 60_000);
        assert_eq!(done.completed_at, Some(t + 360_000));
        assert_eq!(done.break_ends_at, Some(t + 660_000));
        assert_eq!(
            reopened
                .latest_focus_at(t + 1_100_000)
                .unwrap()
                .unwrap()
                .completed_at,
            done.completed_at
        );
        let conn = reopened.connection().unwrap();
        assert_eq!(
            conn.query_row("SELECT ended_at-started_at FROM focus_pauses", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            300_000
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM focus_events", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn countup_uses_wall_time_but_not_pauses() {
        let storage = store("countup-offline");
        let task = task(&storage, "countup", 25, "normal");
        let t = 1_000_000;
        let session = storage.start_task_at(task.id, t).unwrap();
        assert_eq!(
            storage
                .latest_focus_at(t + 86_400_000)
                .unwrap()
                .unwrap()
                .elapsed_milliseconds,
            86_400_000
        );
        storage
            .focus_action_at(session.id, "pause", t + 86_400_000)
            .unwrap();
        let after = storage.latest_focus_at(t + 87_400_000).unwrap().unwrap();
        assert_eq!(after.elapsed_milliseconds, 87_100_000);
        assert_eq!(after.remaining_milliseconds, None);
        assert_eq!(after.status, "focusing");
    }

    #[test]
    fn early_completion_counts_actual_time_once_and_keeps_snapshot() {
        let storage = store("snapshot");
        let mut task = task(&storage, "countdown", 25, "normal");
        let t = Utc::now().timestamp_millis() - 50_000;
        let session = storage.start_task_at(task.id, t).unwrap();
        task = storage
            .save_focus_task(FocusTaskInput {
                id: Some(task.id),
                name: "下次考试".into(),
                timer_kind: "countup".into(),
                duration_minutes: 180,
                mode: "strict".into(),
            })
            .unwrap();
        let current = storage.latest_focus_at(t + 40_000).unwrap().unwrap();
        assert_eq!(current.label, "学习");
        assert_eq!(current.mode, "normal");
        assert_eq!(current.timer_kind, "countdown");
        let done = storage
            .focus_action_at(session.id, "complete", t + 40_000)
            .unwrap();
        let duplicate = storage
            .focus_action_at(session.id, "complete", t + 41_000)
            .unwrap();
        assert_eq!(done.completed_at, duplicate.completed_at);
        assert_eq!(done.elapsed_milliseconds, 40_000);
        let counts = storage.list_focus_tasks().unwrap();
        assert_eq!(counts[0].today_completed, 1);
        assert_eq!(counts[0].today_milliseconds, 40_000);
        storage.delete_focus_task(task.id).unwrap();
        assert!(storage.list_focus_tasks().unwrap().is_empty());
        assert_eq!(
            storage
                .connection()
                .unwrap()
                .query_row(
                    "SELECT task_id FROM focus_sessions WHERE id=?1",
                    params![session.id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            task.id
        );
    }

    #[test]
    fn concurrent_starts_and_actions_have_one_session_and_pause_interval() {
        let storage = store("concurrent");
        let task = task(&storage, "countdown", 25, "normal");
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let storage = storage.clone();
                std::thread::spawn(move || storage.start_focus_task(task.id).unwrap().id)
            })
            .collect();
        let ids: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert!(ids.iter().all(|id| *id == ids[0]));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let storage = storage.clone();
                let id = ids[0];
                std::thread::spawn(move || storage.pause_focus(id).unwrap())
            })
            .collect();
        for worker in workers {
            assert_eq!(worker.join().unwrap().status, "away");
        }
        let conn = storage.connection().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM focus_sessions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM focus_pauses", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn strict_completion_is_immediate_and_no_regular_pause() {
        let storage = store("strict-new");
        let task = task(&storage, "countdown", 25, "strict");
        let t = 1_000_000;
        let s = storage.start_task_at(task.id, t).unwrap();
        assert!(storage.focus_action_at(s.id, "pause", t + 1000).is_err());
        let done = storage.focus_action_at(s.id, "complete", t + 1000).unwrap();
        assert_eq!(done.status, "resting");
        assert_eq!(done.elapsed_milliseconds, 1000);
        assert!(done.pending_end_at.is_none());
        assert_eq!(
            storage
                .focus_action_at(s.id, "skip", t + 2000)
                .unwrap()
                .status,
            "completed"
        );
    }

    #[test]
    fn notifications_are_once_only_and_stale_transitions_are_not_replayed() {
        let storage = store("notifications");
        let task = task(&storage, "countdown", 1, "normal");
        let t = 1_000_000;
        storage.start_task_at(task.id, t).unwrap();
        let events = storage.take_focus_notifications(t + 60_001).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stage, "learning_completed");
        assert!(storage
            .take_focus_notifications(t + 60_002)
            .unwrap()
            .is_empty());
        assert!(storage
            .take_focus_notifications(t + 400_000)
            .unwrap()
            .is_empty());
        assert_eq!(
            storage
                .connection()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM focus_events WHERE consumed=0",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn whitelist_uses_executable_identity_and_delayed_additions() {
        let storage = store("whitelist");
        let app = whitelist(&storage, "application", "C:\\Apps\\Code.EXE");
        assert_eq!(app.target, "code.exe");
        let task = task(&storage, "countdown", 25, "normal");
        storage.start_focus_task(task.id).unwrap();
        let now = Utc::now().timestamp_millis();
        assert!(storage
            .focus_allowed("application", "code.exe", now)
            .unwrap());
        assert!(!storage
            .focus_allowed("application", "code-helper.exe", now)
            .unwrap());
        assert!(storage
            .focus_allowed("application", "explorer.exe", now)
            .unwrap());
        let added = whitelist(&storage, "application", "chat.exe");
        assert!((29_000..=31_000).contains(&(added.available_at - now)));
        assert!(!storage
            .focus_allowed("application", "chat.exe", now)
            .unwrap());
        assert!(storage
            .focus_allowed("application", "chat.exe", added.available_at)
            .unwrap());
        storage.remove_focus_whitelist(app.id).unwrap();
        assert!(!storage
            .focus_allowed("application", "code.exe", now)
            .unwrap());
    }

    #[test]
    fn explorer_default_is_added_once_and_user_removal_is_respected() {
        let storage = store("explorer-default");
        let entries = storage.list_focus_whitelist().unwrap();
        let explorer = entries
            .iter()
            .find(|entry| entry.target == "explorer.exe")
            .unwrap();
        assert_eq!(explorer.display_name, "Windows 资源管理器");
        assert_eq!(explorer.available_at, 0);
        assert!(storage
            .focus_allowed("application", "explorer.exe", Utc::now().timestamp_millis())
            .unwrap());
        storage.remove_focus_whitelist(explorer.id).unwrap();
        storage.initialize().unwrap();
        assert!(!storage
            .focus_allowed("application", "explorer.exe", Utc::now().timestamp_millis())
            .unwrap());
        assert!(!storage
            .list_focus_whitelist()
            .unwrap()
            .iter()
            .any(|entry| entry.target == "explorer.exe"));
    }

    #[test]
    fn explorer_migration_preserves_existing_entry_and_adds_missing_for_upgrade() {
        let storage = store("explorer-upgrade");
        let conn = storage.connection().unwrap();
        conn.execute(
            "DELETE FROM focus_whitelist WHERE target='explorer.exe'",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 6).unwrap();
        drop(conn);
        storage.initialize().unwrap();
        assert_eq!(
            storage
                .list_focus_whitelist()
                .unwrap()
                .iter()
                .filter(|entry| entry.target == "explorer.exe")
                .count(),
            1
        );
        let conn = storage.connection().unwrap();
        conn.execute("UPDATE focus_whitelist SET target='Explorer.EXE',display_name='Existing',path='custom',available_at=123 WHERE target='explorer.exe'", []).unwrap();
        conn.pragma_update(None, "user_version", 6).unwrap();
        drop(conn);
        storage.initialize().unwrap();
        let entries = storage.list_focus_whitelist().unwrap();
        let matching: Vec<_> = entries
            .iter()
            .filter(|entry| entry.target.eq_ignore_ascii_case("explorer.exe"))
            .collect();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].display_name, "Windows 资源管理器");
        assert_eq!(matching[0].path.as_deref(), Some("custom"));
        assert_eq!(matching[0].available_at, 123);
    }

    #[test]
    fn website_whitelist_is_deferred_and_existing_entries_are_retained() {
        let storage = store("website-whitelist-deferred");
        storage.connection().unwrap().execute(
            "INSERT INTO focus_whitelist(target_kind,target,display_name,available_at) VALUES('website','learn.example.com','学习网站',0)",
            [],
        ).unwrap();
        let error = storage
            .save_focus_whitelist(FocusWhitelistInput {
                id: None,
                target_kind: "website".into(),
                target: "other.example.com".into(),
                display_name: "其他网站".into(),
                path: None,
            })
            .unwrap_err();
        assert_eq!(error, "网站白名单暂未启用");
        let entries = storage.list_focus_whitelist().unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.target == "learn.example.com"));
        assert!(storage
            .focus_allowed("website", "unrelated.test", Utc::now().timestamp_millis())
            .unwrap());
    }

    #[test]
    fn focus_exit_without_daily_rule_is_scoped_and_does_not_reset_pause() {
        let storage = store("independent-emergency");
        let task = task(&storage, "countdown", 25, "normal");
        let session = storage.start_focus_task(task.id).unwrap();
        storage
            .connection()
            .unwrap()
            .execute(
                "UPDATE focus_sessions SET pause_used_ms=1234 WHERE id=?1",
                params![session.id],
            )
            .unwrap();
        assert!(storage
            .restriction_for(&activity("chat"))
            .unwrap()
            .is_some());
        storage
            .record_exception(
                "focus".into(),
                "0".into(),
                "application".into(),
                "chat.exe".into(),
                5,
                "紧急使用".into(),
            )
            .unwrap();
        assert!(storage
            .restriction_for(&activity("chat"))
            .unwrap()
            .is_none());
        assert!(storage
            .restriction_for(&activity("other"))
            .unwrap()
            .is_some());
        assert_eq!(
            storage
                .latest_focus()
                .unwrap()
                .unwrap()
                .leave_remaining_milliseconds,
            298_766
        );
        storage.complete_focus(session.id).unwrap();
        storage.start_focus_task(task.id).unwrap();
        assert!(storage
            .restriction_for(&activity("chat"))
            .unwrap()
            .is_some());
    }

    #[test]
    fn schedule_alerts_follow_enabled_boundaries_once_across_restart_and_midnight() {
        let storage = store("schedule-boundary-alerts");
        let day = Local::now().date_naive();
        let start = Local
            .from_local_datetime(&day.and_hms_opt(23, 30, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp_millis();
        let item = storage
            .save_schedule_item(
                None,
                day.format("%Y-%m-%d").to_string(),
                ScheduleImportItem {
                    label: "夜间安排".into(),
                    start: "23:30".into(),
                    duration_minutes: 60,
                    mode: "normal".into(),
                    task_id: None,
                },
            )
            .unwrap();
        assert!(storage
            .take_schedule_notifications(start - 1)
            .unwrap()
            .is_empty());
        let events = storage.take_schedule_notifications(start).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stage, "schedule_start");
        assert!(storage
            .take_schedule_notifications(start + 1)
            .unwrap()
            .is_empty());
        let reopened = Storage::new(storage.path.clone()).unwrap();
        assert!(reopened
            .take_schedule_notifications(start + 2)
            .unwrap()
            .is_empty());
        let end = start + 60 * 60_000;
        let events = reopened.take_schedule_notifications(end).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stage, "schedule_end");
        assert!(reopened
            .take_schedule_notifications(end + 1)
            .unwrap()
            .is_empty());
        reopened.set_schedule_item_enabled(item.id, false).unwrap();
        assert!(reopened
            .take_schedule_notifications(end + 2)
            .unwrap()
            .is_empty());
        assert!(reopened
            .take_schedule_notifications(end + 20_000)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn schedule_alert_preferences_are_independent_and_disabled_items_stay_silent() {
        let storage = store("schedule-alert-preferences");
        let day = Local::now().date_naive();
        let start = Local
            .from_local_datetime(&day.and_hms_opt(9, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp_millis();
        let item = storage
            .save_schedule_item(
                None,
                day.format("%Y-%m-%d").to_string(),
                ScheduleImportItem {
                    label: "安排".into(),
                    start: "09:00".into(),
                    duration_minutes: 60,
                    mode: "normal".into(),
                    task_id: None,
                },
            )
            .unwrap();
        storage
            .save_focus_preferences(FocusPreferences {
                schedule_start_notifications_enabled: false,
                schedule_start_sound_enabled: true,
                schedule_end_notifications_enabled: true,
                schedule_end_sound_enabled: false,
                ..FocusPreferences::default()
            })
            .unwrap();
        let events = storage.take_schedule_notifications(start).unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].notifications_enabled);
        assert!(events[0].sound_enabled);
        let events = storage
            .take_schedule_notifications(start + 3_600_000)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].notifications_enabled);
        assert!(!events[0].sound_enabled);
        storage.set_schedule_item_enabled(item.id, false).unwrap();
        assert!(storage
            .take_schedule_notifications(start)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn schedule_links_task_uses_own_config_and_yields_only_to_learning() {
        let storage = store("schedule-link");
        let linked = task(&storage, "countup", 25, "strict");
        let local = Local::now().date_naive().and_hms_opt(9, 0, 0).unwrap();
        let start = Local
            .from_local_datetime(&local)
            .earliest()
            .unwrap()
            .timestamp_millis();
        let date = local.date().format("%Y-%m-%d").to_string();
        storage
            .save_schedule_item(
                None,
                date.clone(),
                ScheduleImportItem {
                    label: "考试安排".into(),
                    start: "09:00".into(),
                    duration_minutes: 60,
                    mode: "normal".into(),
                    task_id: Some(linked.id),
                },
            )
            .unwrap();
        let ongoing = storage.start_task_at(linked.id, start - 1000).unwrap();
        storage
            .focus_action_at(ongoing.id, "pause", start)
            .unwrap_err();
        assert!(storage
            .activate_due_schedule_at(start + 10_000)
            .unwrap()
            .is_none());
        storage
            .focus_action_at(ongoing.id, "complete", start + 20_000)
            .unwrap();
        let scheduled = storage
            .activate_due_schedule_at(start + 30_000)
            .unwrap()
            .unwrap();
        assert_eq!(scheduled.task_id, Some(linked.id));
        assert_eq!(scheduled.timer_kind, "countdown");
        assert_eq!(scheduled.mode, "normal");
        assert_eq!(scheduled.remaining_milliseconds, Some(3_570_000));
        assert!(storage
            .activate_due_schedule_at(start + 40_000)
            .unwrap()
            .is_none());
        assert!(storage.list_schedule(date).unwrap()[0].started);
    }

    #[test]
    fn missed_schedule_does_not_start_and_legacy_import_does_not_guess_task() {
        let storage = store("schedule-missed");
        task(&storage, "countdown", 25, "normal");
        let parsed:ScheduleImport=serde_json::from_str(r#"{"name":"旧日程","items":[{"label":"学习","start":"09:00","durationMinutes":25,"mode":"normal"}]}"#).unwrap();
        let local = Local::now().date_naive().and_hms_opt(9, 0, 0).unwrap();
        let start = Local
            .from_local_datetime(&local)
            .earliest()
            .unwrap()
            .timestamp_millis();
        let items = storage
            .import_schedule(local.date().format("%Y-%m-%d").to_string(), parsed)
            .unwrap();
        assert!(items[0].task_id.is_none());
        assert!(storage
            .activate_due_schedule_at(start + 25 * 60_000)
            .unwrap()
            .is_none());
    }

    #[test]
    fn future_or_malformed_backup_is_rejected_before_overwriting_live_data() {
        let storage = store("restore-destination");
        let task = task(&storage, "countdown", 25, "normal");
        for malformed in [false, true] {
            let source = store("restore-source");
            let conn = source.connection().unwrap();
            if malformed {
                conn.execute_batch(
                    "DROP TABLE focus_tasks; CREATE TABLE focus_tasks(id INTEGER PRIMARY KEY);",
                )
                .unwrap();
            } else {
                conn.pragma_update(None, "user_version", 99).unwrap();
            }
            drop(conn);
            assert!(storage.restore_from(source.path.clone()).is_err());
            assert_eq!(storage.list_focus_tasks().unwrap()[0].id, task.id);
        }
    }

    #[test]
    fn full_backup_roundtrips_focus_tasks_pauses_appearance_and_legacy_ids() {
        let storage = store("backup-focus");
        let task = task(&storage, "countup", 25, "normal");
        let s = storage.start_focus_task(task.id).unwrap();
        storage.pause_focus(s.id).unwrap();
        whitelist(&storage, "application", "code.exe");
        storage
            .save_focus_appearance(FocusAppearance {
                style: "flip".into(),
                size: "large".into(),
                auto_immerse: true,
            })
            .unwrap();
        let backup = storage.path.with_extension("backup.sqlite");
        storage.backup_to(backup.clone()).unwrap();
        let target = store("restore-focus");
        target.restore_from(backup).unwrap();
        let result = target.focus_workspace().unwrap();
        assert_eq!(result.tasks[0].id, task.id);
        assert_eq!(result.session.unwrap().id, s.id);
        assert_eq!(result.appearance.style, "flip");
        assert!(result.appearance.auto_immerse);
        assert!(result
            .whitelist
            .iter()
            .any(|entry| entry.target == "code.exe"));
        assert!(result
            .whitelist
            .iter()
            .any(|entry| entry.target == "explorer.exe"));
        assert_eq!(
            target
                .connection()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM focus_pauses", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn legacy_migration_and_restore_preserve_original_ids_without_fabricating_completions() {
        let path = std::env::temp_dir().join(format!(
            "kedu-legacy-focus-{}-{}.sqlite",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let now = Utc::now().timestamp_millis();
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE focus_sessions (id INTEGER PRIMARY KEY AUTOINCREMENT,label TEXT NOT NULL,mode TEXT NOT NULL,
            started_at INTEGER NOT NULL,planned_end INTEGER NOT NULL,ended_at INTEGER,
            leave_budget_seconds INTEGER NOT NULL DEFAULT 300,leave_used_seconds INTEGER NOT NULL DEFAULT 0,
            active_leave_started_at INTEGER,pending_end_at INTEGER);
            CREATE TABLE exceptions (id INTEGER PRIMARY KEY AUTOINCREMENT,kind TEXT NOT NULL,target TEXT NOT NULL DEFAULT '',
                started_at INTEGER NOT NULL,ended_at INTEGER NOT NULL,reason TEXT NOT NULL DEFAULT '',rule_name TEXT NOT NULL DEFAULT '',
                subject_kind TEXT NOT NULL DEFAULT '',subject TEXT NOT NULL DEFAULT '',focus_id INTEGER);
            CREATE TABLE scheduled_focus (id INTEGER PRIMARY KEY AUTOINCREMENT,date TEXT NOT NULL,source_name TEXT NOT NULL DEFAULT '',
                label TEXT NOT NULL,start_minute INTEGER NOT NULL,duration_minutes INTEGER NOT NULL,mode TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,started_focus_id INTEGER);
            PRAGMA user_version=2;").unwrap();
        db.execute("INSERT INTO focus_sessions(id,label,mode,started_at,planned_end,ended_at,leave_used_seconds) VALUES(31,'旧完成','normal',?1,?2,?3,50)",params![now-500_000,now-100_000,now-200_000]).unwrap();
        db.execute("INSERT INTO focus_sessions(id,label,mode,started_at,planned_end) VALUES(32,'旧到期','normal',?1,?2)",params![now-300_000,now-100_000]).unwrap();
        db.execute("INSERT INTO focus_sessions(id,label,mode,started_at,planned_end) VALUES(33,'正在学习','normal',?1,?2)",params![now-60_000,now+60_000]).unwrap();
        db.execute("INSERT INTO exceptions(id,kind,target,started_at,ended_at,focus_id) VALUES(45,'focus','2',?1,?2,31)",params![now-400_000,now-200_000]).unwrap();
        db.execute("INSERT INTO scheduled_focus(id,date,label,start_minute,duration_minutes,mode,started_focus_id) VALUES(67,'2026-01-01','旧日程',540,25,'normal',31)",[]).unwrap();
        drop(db);
        let migrated = Storage::new(path).unwrap();
        let db = migrated.connection().unwrap();
        let row:(i64,i64,i64,Option<i64>,Option<i64>,Option<String>)=db.query_row("SELECT id,leave_used_seconds,ended_at,task_id,completed_at,completion_reason FROM focus_sessions WHERE id=31",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
        assert_eq!(row, (31, 50, now - 200_000, None, None, None));
        assert_eq!(
            db.query_row("SELECT phase FROM focus_sessions WHERE id=32", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "completed"
        );
        assert_eq!(
            db.query_row("SELECT focus_id FROM exceptions WHERE id=45", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            31
        );
        assert_eq!(
            db.query_row(
                "SELECT started_focus_id FROM scheduled_focus WHERE id=67",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            31
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM focus_tasks", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM focus_pauses", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(db);
        assert_eq!(migrated.latest_focus().unwrap().unwrap().id, 33);
        migrated.initialize().unwrap();
        assert_eq!(
            migrated
                .connection()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM focus_sessions", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
        let backup = migrated.path.with_extension("backup.sqlite");
        migrated.backup_to(backup.clone()).unwrap();
        let target = store("legacy-restore");
        target.restore_from(backup).unwrap();
        assert_eq!(target.latest_focus().unwrap().unwrap().id, 33);
        assert_eq!(target.list_schedule("2026-01-01".into()).unwrap()[0].id, 67);
    }

    #[test]
    fn paused_task_keeps_priority_and_missed_slot_is_skipped_after_completion() {
        let storage = store("paused-schedule");
        let task = task(&storage, "countdown", 25, "normal");
        let local = Local::now().date_naive().and_hms_opt(10, 0, 0).unwrap();
        let start = Local
            .from_local_datetime(&local)
            .earliest()
            .unwrap()
            .timestamp_millis();
        let date = local.date().format("%Y-%m-%d").to_string();
        storage
            .save_schedule_item(
                None,
                date,
                ScheduleImportItem {
                    label: "日程".into(),
                    start: "10:00".into(),
                    duration_minutes: 1,
                    mode: "strict".into(),
                    task_id: Some(task.id),
                },
            )
            .unwrap();
        let s = storage.start_task_at(task.id, start - 1000).unwrap();
        storage.focus_action_at(s.id, "pause", start).unwrap();
        assert!(storage
            .activate_due_schedule_at(start + 30_000)
            .unwrap()
            .is_none());
        storage
            .focus_action_at(s.id, "complete", start + 61_000)
            .unwrap();
        assert!(storage
            .activate_due_schedule_at(start + 61_000)
            .unwrap()
            .is_none());
        assert_eq!(
            storage.latest_focus_at(start + 61_000).unwrap().unwrap().id,
            s.id
        );
    }

    #[test]
    fn preferences_are_validated_and_only_future_sessions_change_durations() {
        let storage = store("preferences-snapshots");
        assert_eq!(
            storage.focus_preferences().unwrap(),
            FocusPreferences::default()
        );
        let task = task(&storage, "countup", 25, "normal");
        let original = storage.start_task_at(task.id, 1_000_000).unwrap();
        assert_eq!(original.leave_budget_seconds, 300);
        assert_eq!(original.rest_duration_seconds, 300);
        let preferences = FocusPreferences {
            pause_minutes: 2,
            rest_minutes: 7,
            notifications_enabled: false,
            sound_enabled: true,
            ..FocusPreferences::default()
        };
        storage.save_focus_preferences(preferences.clone()).unwrap();
        let rest = storage
            .focus_action_at(original.id, "complete", 1_040_321)
            .unwrap();
        assert_eq!(rest.leave_budget_seconds, 300);
        assert_eq!(
            rest.break_ends_at.unwrap() - rest.completed_at.unwrap(),
            300_000
        );
        storage
            .focus_action_at(original.id, "skip", 1_040_322)
            .unwrap();
        let next = storage.start_task_at(task.id, 1_040_323).unwrap();
        assert_eq!(next.leave_budget_seconds, 120);
        assert_eq!(next.rest_duration_seconds, 420);
        for (pause, rest) in [(-1, 5), (61, 5), (5, -1), (5, 61)] {
            let invalid = FocusPreferences {
                pause_minutes: pause,
                rest_minutes: rest,
                ..preferences.clone()
            };
            assert!(storage.save_focus_preferences(invalid).is_err());
            assert_eq!(storage.focus_preferences().unwrap(), preferences);
        }
    }

    #[test]
    fn customized_pause_budget_supports_long_split_pauses_and_restart() {
        let storage = store("preferences-pause-recovery");
        storage
            .save_focus_preferences(FocusPreferences {
                pause_minutes: 10,
                rest_minutes: 1,
                ..FocusPreferences::default()
            })
            .unwrap();
        let task = task(&storage, "countup", 25, "normal");
        let t = 1_000_000;
        let session = storage.start_task_at(task.id, t).unwrap();
        storage
            .focus_action_at(session.id, "pause", t + 1_000)
            .unwrap();
        let first = storage
            .focus_action_at(session.id, "resume", t + 361_000)
            .unwrap();
        assert_eq!(first.leave_remaining_milliseconds, 240_000);
        assert_eq!(first.elapsed_milliseconds, 1_000);
        storage
            .focus_action_at(session.id, "pause", t + 363_000)
            .unwrap();
        let reopened = Storage::new(storage.path.clone()).unwrap();
        let resumed = reopened.latest_focus_at(t + 604_000).unwrap().unwrap();
        assert_eq!(resumed.status, "focusing");
        assert_eq!(resumed.elapsed_milliseconds, 4_000);
        assert_eq!(resumed.leave_remaining_milliseconds, 0);
        assert_eq!(resumed.leave_used_seconds, 600);
        let completed = reopened
            .focus_action_at(session.id, "complete", t + 604_000)
            .unwrap();
        assert_eq!(completed.break_ends_at, Some(t + 664_000));
        let db = reopened.connection().unwrap();
        let pause: i64 = db
            .query_row(
                "SELECT SUM(ended_at-started_at) FROM focus_pauses WHERE session_id=?1",
                params![session.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pause, 600_000);
    }

    #[test]
    fn zero_pause_and_rest_finish_directly_without_extra_rest_alert() {
        let storage = store("preferences-zero");
        storage
            .save_focus_preferences(FocusPreferences {
                pause_minutes: 0,
                rest_minutes: 0,
                ..FocusPreferences::default()
            })
            .unwrap();
        let task = task(&storage, "countdown", 1, "normal");
        let session = storage.start_task_at(task.id, 1_000_000).unwrap();
        assert!(storage
            .focus_action_at(session.id, "pause", 1_001_000)
            .is_err());
        let finished = storage.latest_focus_at(1_060_000).unwrap().unwrap();
        assert_eq!(finished.status, "completed");
        assert_eq!(finished.rest_duration_seconds, 0);
        let events = storage.take_focus_notifications(1_060_001).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stage, "learning_completed");
        assert!(storage
            .take_focus_notifications(1_060_002)
            .unwrap()
            .is_empty());
        let next = storage.start_task_at(task.id, 1_060_010).unwrap();
        assert_eq!(
            storage
                .focus_action_at(next.id, "complete", 1_060_020)
                .unwrap()
                .status,
            "completed"
        );
    }

    #[test]
    fn notification_and_sound_choices_are_independent_and_events_are_consumed_once() {
        let storage = store("preferences-notifications");
        let task = task(&storage, "countup", 25, "normal");
        for (index, (notifications_enabled, sound_enabled)) in
            [(true, true), (true, false), (false, true), (false, false)]
                .into_iter()
                .enumerate()
        {
            let t = 1_000_000 + index as i64 * 100_000;
            let session = storage.start_task_at(task.id, t).unwrap();
            // Reminder preferences are immediate; duration snapshots are separate.
            storage
                .save_focus_preferences(FocusPreferences {
                    notifications_enabled,
                    sound_enabled,
                    ..FocusPreferences::default()
                })
                .unwrap();
            storage
                .focus_action_at(session.id, "complete", t + 1_000)
                .unwrap();
            let events = storage.take_focus_notifications(t + 1_001).unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].notifications_enabled, notifications_enabled);
            assert_eq!(events[0].sound_enabled, sound_enabled);
            assert!(storage
                .take_focus_notifications(t + 1_002)
                .unwrap()
                .is_empty());
            storage
                .focus_action_at(session.id, "skip", t + 1_003)
                .unwrap();
        }
    }

    #[test]
    fn version_four_backup_gains_default_rest_without_changing_existing_fields() {
        let source = store("preferences-v4-backup");
        let task = task(&source, "countup", 25, "normal");
        let session = source.start_task_at(task.id, 1_000_000).unwrap();
        source
            .focus_action_at(session.id, "complete", 1_040_000)
            .unwrap();
        source
            .focus_action_at(session.id, "skip", 1_040_001)
            .unwrap();
        let db = source.connection().unwrap();
        db.execute_batch(
            "ALTER TABLE focus_sessions DROP COLUMN rest_duration_ms; PRAGMA user_version=4;",
        )
        .unwrap();
        drop(db);
        let target = store("preferences-v4-restore");
        target.restore_from(source.path.clone()).unwrap();
        assert_eq!(
            target.focus_preferences().unwrap(),
            FocusPreferences::default()
        );
        let restored = target.latest_focus_at(1_050_000).unwrap().unwrap();
        assert_eq!(restored.id, session.id);
        assert_eq!(restored.elapsed_milliseconds, 40_000);
        assert_eq!(restored.leave_budget_seconds, 300);
        assert_eq!(restored.rest_duration_seconds, 300);
        assert_eq!(restored.completed_at, Some(1_040_000));
        target.initialize().unwrap();
        assert_eq!(
            target
                .connection()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM focus_sessions", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn new_backup_preserves_preferences_long_pauses_and_duration_snapshots() {
        let source = store("preferences-full-backup");
        let preferences = FocusPreferences {
            pause_minutes: 60,
            rest_minutes: 60,
            notifications_enabled: false,
            sound_enabled: true,
            ..FocusPreferences::default()
        };
        source.save_focus_preferences(preferences.clone()).unwrap();
        let task = task(&source, "countup", 25, "normal");
        let session = source.start_task_at(task.id, 1_000_000).unwrap();
        source
            .focus_action_at(session.id, "pause", 1_001_000)
            .unwrap();
        source
            .focus_action_at(session.id, "resume", 1_401_000)
            .unwrap();
        source
            .focus_action_at(session.id, "complete", 1_402_000)
            .unwrap();
        source
            .focus_action_at(session.id, "skip", 1_402_001)
            .unwrap();
        let backup = source.path.with_extension("backup.sqlite");
        source.backup_to(backup.clone()).unwrap();
        let target = store("preferences-full-restore");
        target.restore_from(backup).unwrap();
        assert_eq!(target.focus_preferences().unwrap(), preferences);
        let restored = target.latest_focus_at(1_402_002).unwrap().unwrap();
        assert_eq!(restored.leave_used_seconds, 400);
        assert_eq!(restored.leave_budget_seconds, 3600);
        assert_eq!(restored.rest_duration_seconds, 3600);
        assert_eq!(restored.elapsed_milliseconds, 2_000);
    }

    #[test]
    fn malformed_preferences_or_budgets_do_not_replace_destination_on_restore() {
        let target = store("preferences-invalid-target");
        let task = task(&target, "countup", 25, "normal");
        for invalid in ["settings", "pause", "rest", "missing_rest"] {
            let source = store("preferences-invalid-source");
            let db = source.connection().unwrap();
            if invalid == "settings" {
                db.execute("INSERT INTO app_settings(key,value) VALUES('focus_preferences','{\"pauseMinutes\":61}')", []).unwrap();
            } else if invalid == "missing_rest" {
                db.execute_batch("ALTER TABLE focus_sessions DROP COLUMN rest_duration_ms;")
                    .unwrap();
            } else {
                let column = if invalid == "pause" {
                    "leave_budget_seconds"
                } else {
                    "rest_duration_ms"
                };
                db.execute(&format!("INSERT INTO focus_sessions(label,mode,started_at,planned_end,{column}) VALUES('bad','normal',1,2,-1)"), []).unwrap();
            }
            drop(db);
            assert!(target.restore_from(source.path.clone()).is_err());
            assert_eq!(target.list_focus_tasks().unwrap()[0].id, task.id);
        }
    }
}

#[derive(Debug, Clone)]
struct Session {
    id: i64,
    label: String,
    mode: String,
    started_at: i64,
    task_id: Option<i64>,
    timer_kind: String,
    duration: i64,
    elapsed: i64,
    phase: String,
    phase_at: i64,
    pause_used: i64,
    completed_at: Option<i64>,
    reason: Option<String>,
    break_end: Option<i64>,
    pause_budget: i64,
    rest_duration: i64,
}

impl Session {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            label: row.get(1)?,
            mode: row.get(2)?,
            started_at: row.get(3)?,
            task_id: row.get(4)?,
            timer_kind: row.get(5)?,
            duration: row.get(6)?,
            elapsed: row.get(7)?,
            phase: row.get(8)?,
            phase_at: row.get(9)?,
            pause_used: row.get(10)?,
            completed_at: row.get(11)?,
            reason: row.get(12)?,
            break_end: row.get(13)?,
            pause_budget: row.get::<_, i64>(14)? * 1000,
            rest_duration: row.get(15)?,
        })
    }

    fn used_pause(&self, now: i64) -> i64 {
        (self.pause_used
            + if self.phase == "away" {
                (now - self.phase_at).max(0)
            } else {
                0
            })
        .clamp(0, self.pause_budget)
    }

    fn learning_elapsed(&self, now: i64) -> i64 {
        let value = self.elapsed
            + if self.phase == "focusing" {
                (now - self.phase_at).max(0)
            } else {
                0
            };
        if self.timer_kind == "countdown" {
            value.clamp(0, self.duration)
        } else {
            value.max(0)
        }
    }

    fn state(&self, now: i64) -> FocusState {
        let now = now.max(self.phase_at);
        let elapsed = self.learning_elapsed(now);
        let leave_remaining = self.pause_budget - self.used_pause(now);
        let remaining = (self.timer_kind == "countdown").then(|| (self.duration - elapsed).max(0));
        FocusState {
            id: self.id,
            label: self.label.clone(),
            mode: self.mode.clone(),
            started_at: self.started_at,
            planned_end: if self.timer_kind == "countup" {
                0
            } else if self.phase == "focusing" {
                self.phase_at + (self.duration - self.elapsed).max(0)
            } else if self.phase == "away" {
                now + remaining.unwrap_or(0)
            } else {
                self.completed_at.unwrap_or(self.started_at + self.duration)
            },
            leave_budget_seconds: self.pause_budget / 1000,
            leave_used_seconds: self.used_pause(now) / 1000,
            leave_remaining_seconds: (leave_remaining + 999) / 1000,
            active_leave_started_at: (self.phase == "away").then_some(self.phase_at),
            pending_end_at: None,
            status: self.phase.clone(),
            task_id: self.task_id,
            timer_kind: self.timer_kind.clone(),
            duration_seconds: self.duration / 1000,
            elapsed_milliseconds: elapsed,
            remaining_milliseconds: remaining,
            leave_remaining_milliseconds: leave_remaining,
            observed_at: now,
            break_ends_at: self.break_end,
            rest_duration_seconds: self.rest_duration / 1000,
            completed_at: self.completed_at,
            completion_reason: self.reason.clone(),
        }
    }

    fn persist(&self, tx: &Transaction<'_>) -> Result<(), String> {
        let planned_end = if self.timer_kind == "countup" {
            0
        } else {
            self.phase_at + (self.duration - self.elapsed).max(0)
        };
        tx.execute(
            "UPDATE focus_sessions SET phase=?2,phase_started_at=?3,elapsed_ms=?4,pause_used_ms=?5,
             completed_at=?6,completion_reason=?7,break_ends_at=?8,
             active_leave_started_at=?9,leave_used_seconds=?10,pending_end_at=NULL,
             planned_end=?11,ended_at=COALESCE(ended_at,?6) WHERE id=?1",
            params![
                self.id,
                self.phase,
                self.phase_at,
                self.elapsed,
                self.pause_used,
                self.completed_at,
                self.reason,
                self.break_end,
                (self.phase == "away").then_some(self.phase_at),
                self.pause_used / 1000,
                planned_end
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn resume(&mut self, tx: &Transaction<'_>, now: i64) -> Result<(), String> {
        let charged = (now - self.phase_at)
            .max(0)
            .min(self.pause_budget - self.pause_used);
        let end = self.phase_at + charged;
        tx.execute(
            "UPDATE focus_pauses SET ended_at=?2 WHERE session_id=?1 AND ended_at IS NULL",
            params![self.id, end],
        )
        .map_err(|e| e.to_string())?;
        self.pause_used += charged;
        self.phase = "focusing".into();
        self.phase_at = end;
        Ok(())
    }

    fn complete(&mut self, tx: &Transaction<'_>, now: i64, reason: &str) -> Result<(), String> {
        self.elapsed = self.learning_elapsed(now);
        if self.phase == "away" {
            self.resume(tx, now)?;
        }
        self.phase = if self.rest_duration == 0 {
            "completed"
        } else {
            "resting"
        }
        .into();
        self.phase_at = now;
        self.completed_at = Some(now);
        self.reason = Some(reason.into());
        self.break_end = Some(now + self.rest_duration);
        tx.execute("INSERT OR IGNORE INTO focus_events(session_id,stage,occurred_at) VALUES(?1,'learning_completed',?2)",
            params![self.id, now]).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn advance(&mut self, tx: &Transaction<'_>, now: i64) -> Result<(), String> {
        let mut changed = false;
        if self.phase == "away" && now >= self.phase_at + self.pause_budget - self.pause_used {
            let resume_at = self.phase_at + self.pause_budget - self.pause_used;
            self.resume(tx, resume_at)?;
            changed = true;
        }
        if self.phase == "focusing"
            && self.timer_kind == "countdown"
            && now >= self.phase_at + (self.duration - self.elapsed).max(0)
        {
            let ended = self.phase_at + (self.duration - self.elapsed).max(0);
            self.complete(tx, ended, "elapsed")?;
            changed = true;
        }
        if self.phase == "resting" && self.break_end.is_some_and(|end| now >= end) {
            self.phase = "completed".into();
            self.phase_at = self.break_end.unwrap();
            tx.execute("INSERT OR IGNORE INTO focus_events(session_id,stage,occurred_at) VALUES(?1,'rest_completed',?2)",
                params![self.id, self.phase_at]).map_err(|e| e.to_string())?;
            changed = true;
        }
        if changed {
            self.persist(tx)?;
        }
        Ok(())
    }
}

fn latest(tx: &Transaction<'_>, now: i64) -> Result<Option<Session>, String> {
    let session = tx.query_row(&format!("{SELECT_SESSION} ORDER BY phase IN ('focusing','away','resting') DESC,started_at DESC,id DESC LIMIT 1"),
        [], Session::read).optional().map_err(|e| e.to_string())?;
    if let Some(mut session) = session {
        session.advance(tx, now)?;
        Ok(Some(session))
    } else {
        Ok(None)
    }
}

fn validate_task(input: &FocusTaskInput) -> Result<(), String> {
    if input.name.trim().is_empty() || input.name.chars().count() > 100 {
        return Err("任务名称需要 1 到 100 个字".into());
    }
    if !matches!(input.timer_kind.as_str(), "countdown" | "countup") {
        return Err("未知计时方式".into());
    }
    if !matches!(input.mode.as_str(), "normal" | "strict") {
        return Err("未知专注模式".into());
    }
    if !(1..=720).contains(&input.duration_minutes) {
        return Err("专注时长必须在 1 到 720 分钟之间".into());
    }
    Ok(())
}

fn validate_preferences(preferences: &FocusPreferences) -> Result<(), String> {
    if !matches!(
        preferences.window_guard_mode.as_str(),
        "reminder" | "minimize"
    ) {
        return Err("请选择有效的窗口拦截方式".into());
    }
    if !(0..=60).contains(&preferences.pause_minutes) {
        return Err("累计暂停时长需要在 0 到 60 分钟之间".into());
    }
    if !(0..=60).contains(&preferences.rest_minutes) {
        return Err("休息时长需要在 0 到 60 分钟之间".into());
    }
    Ok(())
}

fn read_preferences(connection: &Connection) -> Result<FocusPreferences, String> {
    let saved: Option<String> = connection
        .query_row(
            "SELECT value FROM app_settings WHERE key='focus_preferences'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let preferences = saved
        .map(|value| {
            serde_json::from_str::<FocusPreferences>(&value)
                .map_err(|error| format!("专注设置读取失败：{error}"))
        })
        .transpose()?
        .unwrap_or_default();
    validate_preferences(&preferences)?;
    Ok(preferences)
}

fn insert_session(
    tx: &Transaction<'_>,
    task_id: Option<i64>,
    label: &str,
    timer_kind: &str,
    duration: i64,
    mode: &str,
    now: i64,
) -> Result<Session, String> {
    if let Some(previous) = latest(tx, now)? {
        if matches!(previous.phase.as_str(), "focusing" | "away") {
            return Ok(previous);
        }
        if previous.phase == "resting" {
            tx.execute(
                "UPDATE focus_sessions SET phase='completed' WHERE id=?1",
                params![previous.id],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    let preferences = read_preferences(tx)?;
    tx.execute(
        "INSERT INTO focus_sessions
        (label,mode,started_at,planned_end,task_id,timer_kind,duration_ms,phase,phase_started_at,leave_budget_seconds,rest_duration_ms)
        VALUES(?1,?2,?3,?4,?5,?6,?7,'focusing',?3,?8,?9)",
        params![
            label,
            mode,
            now,
            if timer_kind == "countup" {
                0
            } else {
                now + duration
            },
            task_id,
            timer_kind,
            duration,
            preferences.pause_minutes * 60,
            preferences.rest_minutes * 60_000
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.query_row(
        &format!("{SELECT_SESSION} WHERE id=?1"),
        params![tx.last_insert_rowid()],
        Session::read,
    )
    .map_err(|e| e.to_string())
}

impl Storage {
    pub fn focus_preferences(&self) -> Result<FocusPreferences, String> {
        read_preferences(&self.connection()?)
    }

    pub fn save_focus_preferences(
        &self,
        preferences: FocusPreferences,
    ) -> Result<FocusPreferences, String> {
        validate_preferences(&preferences)?;
        let value = serde_json::to_string(&preferences).map_err(|error| error.to_string())?;
        self.connection()?
            .execute(
                "INSERT INTO app_settings(key,value) VALUES('focus_preferences',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![value],
            )
            .map_err(|error| error.to_string())?;
        Ok(preferences)
    }

    pub fn latest_focus(&self) -> Result<Option<FocusState>, String> {
        self.latest_focus_at(Utc::now().timestamp_millis())
    }

    fn latest_focus_at(&self, now: i64) -> Result<Option<FocusState>, String> {
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let state = latest(&tx, now)?.map(|s| s.state(now));
        tx.commit().map_err(|e| e.to_string())?;
        Ok(state)
    }

    pub fn focus_workspace(&self) -> Result<FocusWorkspace, String> {
        Ok(FocusWorkspace {
            session: self.latest_focus()?,
            tasks: self.list_focus_tasks()?,
            whitelist: self.list_focus_whitelist()?,
            appearance: self.focus_appearance()?,
            preferences: self.focus_preferences()?,
        })
    }

    pub fn list_focus_tasks(&self) -> Result<Vec<FocusTask>, String> {
        let day = self.today_start_millis();
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT t.id,t.name,t.timer_kind,t.duration_minutes,t.mode,
             COUNT(s.id),COALESCE(SUM(s.elapsed_ms),0) FROM focus_tasks t
             LEFT JOIN focus_sessions s ON s.task_id=t.id AND s.completed_at>=?1
               AND s.completed_at<=?2 AND s.completion_reason IS NOT NULL
             WHERE t.deleted_at IS NULL GROUP BY t.id ORDER BY t.created_at,t.id",
            )
            .map_err(|e| e.to_string())?;
        let tasks = stmt
            .query_map(params![day, Utc::now().timestamp_millis()], |r| {
                Ok(FocusTask {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    timer_kind: r.get(2)?,
                    duration_minutes: r.get(3)?,
                    mode: r.get(4)?,
                    today_completed: r.get(5)?,
                    today_milliseconds: r.get(6)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(tasks)
    }

    pub fn save_focus_task(&self, input: FocusTaskInput) -> Result<FocusTask, String> {
        validate_task(&input)?;
        let conn = self.connection()?;
        let now = Utc::now().timestamp_millis();
        let id = if let Some(id) = input.id {
            let count = conn
                .execute(
                    "UPDATE focus_tasks SET name=?2,timer_kind=?3,duration_minutes=?4,
                 mode=?5,updated_at=?6 WHERE id=?1 AND deleted_at IS NULL",
                    params![
                        id,
                        input.name.trim(),
                        input.timer_kind,
                        input.duration_minutes,
                        input.mode,
                        now
                    ],
                )
                .map_err(|e| e.to_string())?;
            if count == 0 {
                return Err("任务不存在".into());
            }
            id
        } else {
            conn.execute("INSERT INTO focus_tasks(name,timer_kind,duration_minutes,mode,created_at,updated_at)
                VALUES(?1,?2,?3,?4,?5,?5)",
                params![input.name.trim(),input.timer_kind,input.duration_minutes,input.mode,now]).map_err(|e| e.to_string())?;
            conn.last_insert_rowid()
        };
        self.list_focus_tasks()?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| "无法读取任务".into())
    }

    pub fn delete_focus_task(&self, id: i64) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE focus_tasks SET deleted_at=?2 WHERE id=?1",
                params![id, Utc::now().timestamp_millis()],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn start_focus_task(&self, task_id: i64) -> Result<FocusState, String> {
        self.start_task_at(task_id, Utc::now().timestamp_millis())
    }

    fn start_task_at(&self, task_id: i64, now: i64) -> Result<FocusState, String> {
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let input = tx.query_row("SELECT name,timer_kind,duration_minutes,mode FROM focus_tasks WHERE id=?1 AND deleted_at IS NULL",
            params![task_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?)))
            .optional().map_err(|e| e.to_string())?.ok_or("任务不存在")?;
        let state = insert_session(
            &tx,
            Some(task_id),
            &input.0,
            &input.1,
            input.2 * 60_000,
            &input.3,
            now,
        )?
        .state(now);
        tx.commit().map_err(|e| e.to_string())?;
        Ok(state)
    }

    pub(super) fn start_ad_hoc_focus(
        &self,
        label: String,
        minutes: i64,
        mode: String,
    ) -> Result<FocusState, String> {
        validate_task(&FocusTaskInput {
            id: None,
            name: label.clone(),
            timer_kind: "countdown".into(),
            duration_minutes: minutes,
            mode: mode.clone(),
        })?;
        let now = Utc::now().timestamp_millis();
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let state = insert_session(
            &tx,
            None,
            label.trim(),
            "countdown",
            minutes * 60_000,
            &mode,
            now,
        )?
        .state(now);
        tx.commit().map_err(|e| e.to_string())?;
        Ok(state)
    }

    fn focus_action_at(&self, id: i64, action: &str, now: i64) -> Result<FocusState, String> {
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut session = tx
            .query_row(
                &format!("{SELECT_SESSION} WHERE id=?1"),
                params![id],
                Session::read,
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("专注记录不存在")?;
        let now = now.max(session.phase_at);
        session.advance(&tx, now)?;
        match action {
            "pause" if session.phase == "focusing" => {
                if session.mode == "strict" {
                    return Err("严格模式不提供普通暂停，可提前完成".into());
                }
                if session.pause_used >= session.pause_budget {
                    return Err("本轮累计暂停时长已用完".into());
                }
                session.elapsed = session.learning_elapsed(now);
                session.phase = "away".into();
                session.phase_at = now;
                tx.execute(
                    "INSERT OR IGNORE INTO focus_pauses(session_id,started_at) VALUES(?1,?2)",
                    params![id, now],
                )
                .map_err(|e| e.to_string())?;
            }
            "resume" if session.phase == "away" => session.resume(&tx, now)?,
            "complete" if matches!(session.phase.as_str(), "focusing" | "away") => {
                session.complete(&tx, now, "manual")?
            }
            "skip" if session.phase == "resting" => {
                session.phase = "completed".into();
                session.phase_at = now;
            }
            _ => {}
        }
        session.persist(&tx)?;
        let state = session.state(now);
        tx.commit().map_err(|e| e.to_string())?;
        Ok(state)
    }

    pub fn pause_focus(&self, id: i64) -> Result<FocusState, String> {
        self.focus_action_at(id, "pause", Utc::now().timestamp_millis())
    }
    pub fn resume_focus(&self, id: i64) -> Result<FocusState, String> {
        self.focus_action_at(id, "resume", Utc::now().timestamp_millis())
    }
    pub fn complete_focus(&self, id: i64) -> Result<FocusState, String> {
        self.focus_action_at(id, "complete", Utc::now().timestamp_millis())
    }
    pub fn skip_focus_break(&self, id: i64) -> Result<FocusState, String> {
        self.focus_action_at(id, "skip", Utc::now().timestamp_millis())
    }

    pub fn take_focus_notifications(&self, now: i64) -> Result<Vec<FocusNotification>, String> {
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        latest(&tx, now)?;
        let preferences = read_preferences(&tx)?;
        let events = {
            let mut stmt=tx.prepare("SELECT e.session_id,e.stage,s.label,e.occurred_at FROM focus_events e
                JOIN focus_sessions s ON s.id=e.session_id WHERE e.consumed=0 AND e.occurred_at BETWEEN ?1 AND ?2
                ORDER BY e.occurred_at").map_err(|e|e.to_string())?;
            let rows = stmt
                .query_map(params![now - 10_000, now], |r| {
                    Ok(FocusNotification {
                        session_id: r.get(0)?,
                        stage: r.get(1)?,
                        label: r.get(2)?,
                        occurred_at: r.get(3)?,
                        notifications_enabled: preferences.notifications_enabled,
                        sound_enabled: preferences.sound_enabled,
                    })
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        tx.execute(
            "UPDATE focus_events SET consumed=1 WHERE consumed=0 AND occurred_at<=?1",
            params![now],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(events)
    }

    /// Consume only recent scheduled boundaries, once per schedule, stage and
    /// timestamp. Older boundaries are intentionally not replayed after sleep.
    pub fn take_schedule_notifications(
        &self,
        now: i64,
    ) -> Result<Vec<ScheduleNotification>, String> {
        let local = Utc
            .timestamp_millis_opt(now)
            .single()
            .ok_or("无效时间")?
            .with_timezone(&Local);
        let today = local.date_naive();
        let dates = [
            today.format("%Y-%m-%d").to_string(),
            (today - chrono::Duration::days(1))
                .format("%Y-%m-%d")
                .to_string(),
        ];
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let preferences = read_preferences(&tx)?;
        let rows = {
            let mut stmt = tx.prepare("SELECT id,date,label,start_minute,duration_minutes,started_focus_id FROM scheduled_focus WHERE enabled=1 AND date IN (?1,?2)").map_err(|e| e.to_string())?;
            let result = stmt
                .query_map(params![dates[0], dates[1]], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            result
        };
        let mut events = Vec::new();
        for (id, date, label, minute, duration, session_id) in rows {
            let naive = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                .map_err(|e| e.to_string())?
                .and_hms_opt((minute / 60) as u32, (minute % 60) as u32, 0)
                .ok_or("日程时间无效")?;
            let Some(start) = Local.from_local_datetime(&naive).earliest() else {
                continue;
            };
            let start = start.timestamp_millis();
            for (stage, occurred_at, notifications_enabled, sound_enabled) in [
                (
                    "schedule_start",
                    start,
                    preferences.schedule_start_notifications_enabled,
                    preferences.schedule_start_sound_enabled,
                ),
                (
                    "schedule_end",
                    start + duration * 60_000,
                    preferences.schedule_end_notifications_enabled,
                    preferences.schedule_end_sound_enabled,
                ),
            ] {
                if occurred_at > now || occurred_at < now - 10_000 {
                    continue;
                }
                let inserted = tx.execute("INSERT OR IGNORE INTO schedule_alert_events(schedule_id,stage,occurred_at) VALUES(?1,?2,?3)", params![id,stage,occurred_at]).map_err(|e| e.to_string())?;
                if inserted != 0 && (notifications_enabled || sound_enabled) {
                    events.push(ScheduleNotification {
                        session_id,
                        stage,
                        label: label.clone(),
                        occurred_at,
                        notifications_enabled,
                        sound_enabled,
                    });
                }
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(events)
    }

    pub fn list_focus_whitelist(&self) -> Result<Vec<FocusWhitelistEntry>, String> {
        let conn = self.connection()?;
        let mut stmt=conn.prepare("SELECT id,target_kind,target,display_name,path,available_at FROM focus_whitelist ORDER BY target_kind,display_name,id").map_err(|e|e.to_string())?;
        let values = stmt
            .query_map([], |r| {
                Ok(FocusWhitelistEntry {
                    id: r.get(0)?,
                    target_kind: r.get(1)?,
                    target: r.get(2)?,
                    display_name: r.get(3)?,
                    path: r.get(4)?,
                    available_at: r.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(values)
    }

    pub fn save_focus_whitelist(
        &self,
        input: FocusWhitelistInput,
    ) -> Result<FocusWhitelistEntry, String> {
        let target = match input.target_kind.as_str() {
            "application" => {
                let stem = normalize_application_subject(&input.target);
                if stem.is_empty() || stem.contains(['*', '?', ';', ',', ':', '"']) {
                    return Err("请选择有效的应用".into());
                }
                crate::app_identity::canonical_executable(&input.target)
            }
            "website" => return Err("网站白名单暂未启用".into()),
            _ => return Err("白名单只支持应用".into()),
        };
        if target.is_empty() || input.display_name.trim().is_empty() {
            return Err("请选择白名单目标".into());
        }
        let now = Utc::now().timestamp_millis();
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let active =
            latest(&tx, now)?.is_some_and(|s| matches!(s.phase.as_str(), "focusing" | "away"));
        let available = now + if active { 30_000 } else { 0 };
        if let Some(id) = input.id {
            tx.execute(
                "DELETE FROM focus_whitelist WHERE id=?1 AND (target_kind!=?2 OR target!=?3)",
                params![id, input.target_kind, target],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute("INSERT INTO focus_whitelist(target_kind,target,display_name,path,available_at) VALUES(?1,?2,?3,?4,?5)
            ON CONFLICT(target_kind,target) DO UPDATE SET display_name=excluded.display_name,path=excluded.path",
            params![input.target_kind,target,input.display_name.trim(),input.path,available]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        self.list_focus_whitelist()?
            .into_iter()
            .find(|v| v.target_kind == input.target_kind && v.target == target)
            .ok_or_else(|| "白名单保存失败".into())
    }

    pub fn remove_focus_whitelist(&self, id: i64) -> Result<(), String> {
        self.connection()?
            .execute("DELETE FROM focus_whitelist WHERE id=?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn focus_appearance(&self) -> Result<FocusAppearance, String> {
        let raw = self
            .connection()?
            .query_row(
                "SELECT value FROM app_settings WHERE key='focus_appearance'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        match raw {
            Some(raw) => serde_json::from_str(&raw).map_err(|e| e.to_string()),
            None => Ok(FocusAppearance::default()),
        }
    }

    pub fn save_focus_appearance(
        &self,
        appearance: FocusAppearance,
    ) -> Result<FocusAppearance, String> {
        if !matches!(appearance.style.as_str(), "normal" | "digital" | "flip")
            || !matches!(appearance.size.as_str(), "small" | "normal" | "large")
        {
            return Err("未知计时器外观".into());
        }
        self.connection()?.execute("INSERT INTO app_settings(key,value) VALUES('focus_appearance',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![serde_json::to_string(&appearance).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        Ok(appearance)
    }

    pub(super) fn focus_allowed(
        &self,
        kind: &str,
        subject: &str,
        now: i64,
    ) -> Result<bool, String> {
        if kind == "website" {
            return Ok(true);
        }
        if kind == "application"
            && matches!(
                normalize_application_subject(subject).as_str(),
                "kedu"
                    | "shellexperiencehost"
                    | "startmenuexperiencehost"
                    | "searchhost"
                    | "searchapp"
                    | "lockapp"
                    | "logonui"
            )
        {
            return Ok(true);
        }
        Ok(self.list_focus_whitelist()?.iter().any(|entry| {
            entry.target_kind == kind
                && entry.available_at <= now
                && crate::app_identity::canonical_executable(subject)
                    == crate::app_identity::canonical_executable(&entry.target)
        }))
    }

    pub(super) fn activate_due_schedule_at(&self, now: i64) -> Result<Option<FocusState>, String> {
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if latest(&tx, now)?.is_some_and(|s| matches!(s.phase.as_str(), "focusing" | "away")) {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(None);
        }
        let local = Utc
            .timestamp_millis_opt(now)
            .single()
            .ok_or("无效时间")?
            .with_timezone(&Local);
        let date = local.date_naive();
        let dates = [
            date.format("%Y-%m-%d").to_string(),
            (date - chrono::Duration::days(1))
                .format("%Y-%m-%d")
                .to_string(),
        ];
        let rows = {
            let mut stmt=tx.prepare("SELECT id,date,label,start_minute,duration_minutes,mode,task_id FROM scheduled_focus
                WHERE date IN (?1,?2) AND enabled=1 AND started_focus_id IS NULL ORDER BY date,start_minute,id").map_err(|e|e.to_string())?;
            let values = stmt
                .query_map(params![dates[0], dates[1]], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<i64>>(6)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            values
        };
        let mut result = None;
        for (id, date, label, minute, duration, mode, task_id) in rows {
            let naive = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                .map_err(|e| e.to_string())?
                .and_hms_opt((minute / 60) as u32, (minute % 60) as u32, 0)
                .ok_or("日程时间无效")?;
            let Some(start) = Local.from_local_datetime(&naive).earliest() else {
                continue;
            };
            let start = start.timestamp_millis();
            let end = start + duration * 60_000;
            if start <= now && now < end {
                let session =
                    insert_session(&tx, task_id, &label, "countdown", end - now, &mode, now)?;
                tx.execute(
                    "UPDATE scheduled_focus SET started_focus_id=?2 WHERE id=?1",
                    params![id, session.id],
                )
                .map_err(|e| e.to_string())?;
                result = Some(session.state(now));
                break;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result)
    }
}
