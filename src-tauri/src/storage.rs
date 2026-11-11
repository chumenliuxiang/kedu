use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use chrono::{Local, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use rusqlite::{backup::Backup, params, Connection, OptionalExtension};

use crate::domain::{
    ActivityIdentity, ActivityStatistics, AppSettings, AppUsage, ClassificationRule, Dashboard,
    ExceptionUsage, FocusState, LimitRule, NewClassificationRule, NewLimitRule, RestrictionCause,
    RestrictionNotice, RuleChangeResult, ScheduleImport, ScheduleImportItem, ScheduleItem,
    TargetApplication, TargetCatalog, TimelineItem, TrendPoint, TrendSeries, WebActivity,
};

fn normalize_domain(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .trim_end_matches('.')
        .trim_start_matches("www.")
        .to_string()
}

fn normalize_application_subject(value: &str) -> String {
    value
        .trim()
        .replace('/', "\\")
        .rsplit('\\')
        .next()
        .unwrap_or_default()
        .to_lowercase()
        .trim_end_matches(".exe")
        .to_string()
}

fn domain_matches(domain: &str, pattern: &str) -> bool {
    let domain = normalize_domain(domain);
    let pattern = normalize_domain(pattern);
    !pattern.is_empty() && (domain == pattern || domain.ends_with(&format!(".{pattern}")))
}

fn group_names(value: &str) -> impl Iterator<Item = &str> {
    value
        .split([';', ',', '；', '，'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
}

fn listed_identity_matches(list: &str, identity: &str) -> bool {
    let identity = identity.to_lowercase();
    group_names(list).any(|pattern| identity.contains(&pattern.to_lowercase()))
}

fn rule_belongs_to_group(rule: &LimitRule, group: &LimitRule) -> bool {
    let target = group.target_pattern.trim();
    group_names(&rule.group_name).any(|name| name.eq_ignore_ascii_case(target))
}

fn application_matches_rule(activity: &ActivityIdentity, rule: &LimitRule) -> bool {
    crate::app_identity::matches(
        &activity.process_path,
        &activity.app_name,
        &rule.target_pattern,
    )
}

fn merged_seconds(mut intervals: Vec<(i64, i64)>) -> i64 {
    intervals.sort_unstable_by_key(|interval| interval.0);
    let mut total = 0i64;
    let mut current: Option<(i64, i64)> = None;
    for (start, end) in intervals {
        let Some((current_start, current_end)) = current else {
            current = Some((start, end));
            continue;
        };
        if start <= current_end {
            current = Some((current_start, current_end.max(end)));
        } else {
            total += current_end.saturating_sub(current_start);
            current = Some((start, end));
        }
    }
    if let Some((start, end)) = current {
        total += end.saturating_sub(start);
    }
    total / 1000
}

#[path = "application_icons.rs"]
mod application_icons;
#[path = "category_exchange.rs"]
pub(crate) mod category_exchange;
#[path = "category_management.rs"]
pub(crate) mod category_management;
#[cfg(test)]
#[path = "category_tests.rs"]
mod category_tests;
#[path = "focus_engine.rs"]
mod focus_engine;
#[path = "focus_statistics.rs"]
pub(crate) mod focus_statistics;
#[path = "limit_reminders.rs"]
mod limit_reminders;
#[path = "statistics.rs"]
mod statistics;
pub use statistics::UsageInsights;
#[path = "theme.rs"]
mod theme;
#[path = "timeline_annotations.rs"]
mod timeline_annotations;
#[path = "web_recording.rs"]
mod web_recording;

#[derive(Clone)]
pub struct Storage {
    path: PathBuf,
}

impl Storage {
    pub fn new(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let storage = Self { path };
        storage.initialize()?;
        Ok(storage)
    }

    pub(crate) fn connection(&self) -> Result<Connection, String> {
        let connection = Connection::open(&self.path).map_err(|error| error.to_string())?;
        connection
            .busy_timeout(std::time::Duration::from_secs(10))
            .map_err(|error| error.to_string())?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| error.to_string())?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|error| error.to_string())?;
        Ok(connection)
    }

    pub fn latest_application_path(&self, target: &str) -> Result<Option<String>, String> {
        let connection = self.connection()?;
        for candidate in target
            .split([';', ','])
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
        {
            let normalized = normalize_application_subject(candidate);
            if normalized.is_empty() {
                continue;
            }
            let contains = format!("%{normalized}%");
            let executable = format!("{normalized}.exe");
            let path_suffix = format!("%\\{executable}");
            let path = connection
                .query_row(
                    "SELECT process_path
                     FROM activity_segments
                     WHERE process_path <> ''
                       AND (lower(app_name) = ?1
                            OR lower(app_name) LIKE ?2
                            OR lower(process_path) = ?3
                            OR replace(lower(process_path), '/', '\\') LIKE ?4)
                     ORDER BY CASE
                                WHEN lower(app_name) = ?1 THEN 0
                                WHEN lower(process_path) = ?3
                                  OR replace(lower(process_path), '/', '\\') LIKE ?4 THEN 1
                                ELSE 2
                              END,
                              ended_at DESC
                     LIMIT 1",
                    params![normalized, contains, executable, path_suffix],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?;
            if path.is_some() {
                return Ok(path);
            }
        }
        Ok(None)
    }

    pub fn target_catalog(&self) -> Result<TargetCatalog, String> {
        let connection = self.connection()?;
        let mut app_statement = connection
            .prepare(
                "SELECT app_name, process_path, MAX(ended_at) AS last_seen
                 FROM activity_segments
                 WHERE process_path <> ''
                 GROUP BY lower(process_path)
                 ORDER BY last_seen DESC",
            )
            .map_err(|error| error.to_string())?;
        let applications = app_statement
            .query_map([], |row| {
                let display_name = row.get::<_, String>(0)?;
                let path = row.get::<_, String>(1)?;
                let executable_name = Path::new(&path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(&display_name)
                    .to_string();
                Ok(TargetApplication {
                    display_name,
                    executable_name,
                    path,
                    source: "history".into(),
                    last_seen_at: Some(row.get(2)?),
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;

        let mut site_statement = connection
            .prepare(
                "SELECT domain, MAX(ended_at) AS last_seen
                 FROM web_activity_segments
                 WHERE trim(domain) <> ''
                 GROUP BY lower(domain)
                 ORDER BY last_seen DESC",
            )
            .map_err(|error| error.to_string())?;
        let websites = site_statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|domain| normalize_domain(&domain))
            .filter(|domain| !domain.is_empty())
            .collect();

        let mut groups = BTreeSet::new();
        let mut group_statement = connection
            .prepare("SELECT target_kind, target_pattern, group_name FROM limit_rules")
            .map_err(|error| error.to_string())?;
        let rows = group_statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        for (kind, target, memberships) in rows {
            if kind == "group" && !target.trim().is_empty() {
                groups.insert(target.trim().to_string());
            }
            for group in group_names(&memberships) {
                if !group.trim().is_empty() {
                    groups.insert(group.trim().to_string());
                }
            }
        }

        Ok(TargetCatalog {
            applications,
            websites,
            groups: groups.into_iter().collect(),
        })
    }

    pub fn backup_to(&self, destination: PathBuf) -> Result<(), String> {
        if destination == self.path {
            return Err("备份文件不能覆盖正在使用的数据库".into());
        }
        if destination.exists() {
            fs::remove_file(&destination).map_err(|error| error.to_string())?;
        }
        let source = self.connection()?;
        let mut target = Connection::open(destination).map_err(|error| error.to_string())?;
        let backup = Backup::new(&source, &mut target).map_err(|error| error.to_string())?;
        backup
            .run_to_completion(32, std::time::Duration::from_millis(20), None)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn activity_date_range(&self) -> Result<(Option<i64>, Option<i64>), String> {
        self.connection()?
            .query_row(
                "SELECT
                    (SELECT MIN(value) FROM (
                        SELECT MIN(started_at) AS value FROM activity_segments
                        UNION ALL SELECT MIN(started_at) FROM web_activity_segments)),
                    (SELECT MAX(value) FROM (
                        SELECT MAX(ended_at) AS value FROM activity_segments
                        UNION ALL SELECT MAX(ended_at) FROM web_activity_segments))",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| error.to_string())
    }

    pub fn restore_from(&self, source_path: PathBuf) -> Result<(), String> {
        if source_path == self.path {
            return Err("当前数据库无需恢复".into());
        }
        let source =
            Connection::open_with_flags(&source_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| error.to_string())?;
        for table in [
            "activity_segments",
            "focus_sessions",
            "limit_rules",
            "exceptions",
        ] {
            let exists = source
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    params![table],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| error.to_string())?
                != 0;
            if !exists {
                return Err(format!("所选文件不是有效的刻度备份：缺少 {table}"));
            }
        }
        let version: i64 = source
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if version > 9 {
            return Err("备份来自更新版本的刻度，请更新软件后再恢复".into());
        }
        if version >= 5 {
            // Only older schemas may acquire default duration snapshots. A new
            // backup missing them is damaged, not a legacy five-minute session.
            source
                .prepare("SELECT leave_budget_seconds,rest_duration_ms FROM focus_sessions")
                .map_err(|error| format!("备份缺少专注时长快照：{error}"))?;
        }
        if version >= 6 {
            category_management::validate(&source)?;
            category_exchange::validate(&source)?;
        }
        if version >= 8 {
            timeline_annotations::validate(&source)?;
        }
        if version >= 9 {
            application_icons::validate(&source)?;
        }
        let integrity: String = source
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if integrity != "ok" {
            return Err(format!("备份完整性检查失败：{integrity}"));
        }
        // Migrate a consistent snapshot first. A malformed or incompatible backup
        // must never overwrite the working database before we discover the error.
        let stage_path = self.path.with_file_name(format!(
            ".kedu-restore-{}-{}.sqlite",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let result = (|| {
            let mut stage_connection =
                Connection::open(&stage_path).map_err(|error| error.to_string())?;
            {
                let backup = Backup::new(&source, &mut stage_connection)
                    .map_err(|error| error.to_string())?;
                backup
                    .run_to_completion(128, std::time::Duration::from_millis(5), None)
                    .map_err(|error| error.to_string())?;
            }
            drop(stage_connection);
            let stage = Storage {
                path: stage_path.clone(),
            };
            stage.initialize()?;
            let validated = stage.connection()?;
            let integrity: String = validated
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            if integrity != "ok" {
                return Err(format!("迁移后的备份检查失败：{integrity}"));
            }
            // Prepare every new state query, even when the backup declares the
            // current version. Version metadata alone is not proof of its shape.
            validated
                .prepare(
                    "SELECT task_id,phase,phase_started_at,pause_used_ms,elapsed_ms,
                duration_ms,timer_kind,completed_at,completion_reason,break_ends_at,
                leave_budget_seconds,rest_duration_ms FROM focus_sessions",
                )
                .map_err(|error| error.to_string())?;
            stage.settings()?;
            web_recording::validate(&validated)?;
            limit_reminders::validate(&validated)?;
            category_management::validate(&validated)?;
            category_exchange::validate(&validated)?;
            timeline_annotations::validate(&validated)?;
            stage.list_schedule(Local::now().format("%Y-%m-%d").to_string())?;
            let invalid: bool = validated.query_row(
                "SELECT EXISTS(SELECT 1 FROM focus_sessions WHERE
                    phase NOT IN ('focusing','away','resting','completed') OR
                    timer_kind NOT IN ('countdown','countup') OR elapsed_ms<0 OR
                    leave_budget_seconds NOT BETWEEN 0 AND 3600 OR
                    rest_duration_ms NOT BETWEEN 0 AND 3600000 OR
                    pause_used_ms NOT BETWEEN 0 AND leave_budget_seconds*1000 OR
                    (task_id IS NOT NULL AND task_id NOT IN (SELECT id FROM focus_tasks)))
                 OR EXISTS(SELECT 1 FROM focus_pauses WHERE session_id NOT IN (SELECT id FROM focus_sessions))
                 OR EXISTS(SELECT 1 FROM focus_events WHERE session_id NOT IN (SELECT id FROM focus_sessions))
                 OR EXISTS(SELECT 1 FROM scheduled_focus WHERE task_id IS NOT NULL AND task_id NOT IN (SELECT id FROM focus_tasks))",
                [], |row| row.get(0)).map_err(|error| error.to_string())?;
            if invalid {
                return Err("备份中的专注记录或任务关联无效".into());
            }
            stage.focus_workspace()?;
            let mut target = self.connection()?;
            let backup = Backup::new(&validated, &mut target).map_err(|error| error.to_string())?;
            backup
                .run_to_completion(128, std::time::Duration::from_millis(5), None)
                .map_err(|error| error.to_string())
        })();
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", stage_path.to_string_lossy()));
        }
        result
    }

    pub fn export_csv(&self, destination: PathBuf, start: i64, end: i64) -> Result<(), String> {
        if end <= start {
            return Err("导出结束时间必须晚于开始时间".into());
        }
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT kind, target, title, started_at, ended_at, idle FROM (
                    SELECT 'application' AS kind, app_name AS target, title, started_at, ended_at, idle
                    FROM activity_segments WHERE ended_at >= ?1 AND started_at <= ?2
                    UNION ALL
                    SELECT 'website', domain, title, started_at, ended_at, idle
                    FROM web_activity_segments WHERE ended_at >= ?1 AND started_at <= ?2
                 ) ORDER BY started_at",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![start, end], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)? != 0,
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let mut csv = String::from("\u{feff}类型,目标,标题,开始时间,结束时间,有效秒数,闲置\r\n");
        for (kind, target, title, row_start, row_end, idle) in rows {
            let clipped_start = row_start.max(start);
            let clipped_end = row_end.min(end);
            let started = Utc
                .timestamp_millis_opt(clipped_start)
                .single()
                .map(|value| value.with_timezone(&Local).to_rfc3339())
                .unwrap_or_default();
            let ended = Utc
                .timestamp_millis_opt(clipped_end)
                .single()
                .map(|value| value.with_timezone(&Local).to_rfc3339())
                .unwrap_or_default();
            let escape = |value: &str| format!("\"{}\"", value.replace('"', "\"\""));
            csv.push_str(&format!(
                "{},{},{},{},{},{},{}\r\n",
                escape(&kind),
                escape(&target),
                escape(&title),
                escape(&started),
                escape(&ended),
                if idle {
                    0
                } else {
                    clipped_end.saturating_sub(clipped_start) / 1000
                },
                if idle { "是" } else { "否" }
            ));
        }
        fs::write(destination, csv).map_err(|error| error.to_string())
    }

    fn initialize(&self) -> Result<(), String> {
        let mut connection = self.connection()?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if version > 9 {
            return Err("数据库来自更新版本的刻度，请更新软件后重试".into());
        }
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS activity_segments (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    started_at INTEGER NOT NULL,
                    ended_at INTEGER NOT NULL,
                    app_name TEXT NOT NULL,
                    process_path TEXT NOT NULL DEFAULT '',
                    title TEXT NOT NULL DEFAULT '',
                    idle INTEGER NOT NULL DEFAULT 0,
                    idle_exempt INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS idx_activity_time
                    ON activity_segments(started_at, ended_at);
                CREATE INDEX IF NOT EXISTS idx_activity_latest
                    ON activity_segments(ended_at DESC, started_at DESC, id DESC);
                CREATE TABLE IF NOT EXISTS focus_sessions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    label TEXT NOT NULL,
                    mode TEXT NOT NULL CHECK(mode IN ('normal', 'strict')),
                    started_at INTEGER NOT NULL,
                    planned_end INTEGER NOT NULL,
                    ended_at INTEGER,
                    leave_budget_seconds INTEGER NOT NULL DEFAULT 300,
                    leave_used_seconds INTEGER NOT NULL DEFAULT 0,
                    active_leave_started_at INTEGER,
                    pending_end_at INTEGER
                );
                CREATE TABLE IF NOT EXISTS limit_rules (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    target_kind TEXT NOT NULL,
                    target_pattern TEXT NOT NULL,
                    group_name TEXT NOT NULL DEFAULT '',
                    category TEXT NOT NULL DEFAULT '',
                    daily_limit_minutes INTEGER NOT NULL,
                    enabled INTEGER NOT NULL DEFAULT 1
                );
                CREATE TABLE IF NOT EXISTS exceptions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    kind TEXT NOT NULL,
                    target TEXT NOT NULL DEFAULT '',
                    started_at INTEGER NOT NULL,
                    ended_at INTEGER NOT NULL,
                    reason TEXT NOT NULL DEFAULT '',
                    rule_name TEXT NOT NULL DEFAULT '',
                    subject_kind TEXT NOT NULL DEFAULT '',
                    subject TEXT NOT NULL DEFAULT '',
                    focus_id INTEGER
                );
                CREATE TABLE IF NOT EXISTS web_activity_segments (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    started_at INTEGER NOT NULL,
                    ended_at INTEGER NOT NULL,
                    domain TEXT NOT NULL,
                    title TEXT NOT NULL DEFAULT '',
                    media_active INTEGER NOT NULL DEFAULT 0,
                    idle INTEGER NOT NULL DEFAULT 0,
                    idle_exempt INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS idx_web_activity_time
                    ON web_activity_segments(started_at, ended_at);
                CREATE INDEX IF NOT EXISTS idx_web_activity_latest
                    ON web_activity_segments(ended_at DESC, started_at DESC, id DESC);
                CREATE TABLE IF NOT EXISTS scheduled_focus (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    date TEXT NOT NULL,
                    source_name TEXT NOT NULL DEFAULT '',
                    label TEXT NOT NULL,
                    start_minute INTEGER NOT NULL,
                    duration_minutes INTEGER NOT NULL,
                    mode TEXT NOT NULL CHECK(mode IN ('normal', 'strict')),
                    enabled INTEGER NOT NULL DEFAULT 1,
                    started_focus_id INTEGER
                );
                CREATE INDEX IF NOT EXISTS idx_scheduled_focus_date
                    ON scheduled_focus(date, start_minute);
                CREATE TABLE IF NOT EXISTS pending_rule_changes (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    rule_id INTEGER NOT NULL,
                    enabled INTEGER NOT NULL,
                    apply_at INTEGER NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS classification_rules (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    target_kind TEXT NOT NULL CHECK(target_kind IN ('application', 'website', 'title')),
                    target_pattern TEXT NOT NULL,
                    category TEXT NOT NULL,
                    enabled INTEGER NOT NULL DEFAULT 1
                );
                CREATE TABLE IF NOT EXISTS app_settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                INSERT OR IGNORE INTO app_settings (key, value) VALUES
                    ('idle_minutes', '5'),
                    ('count_media_while_idle', 'true'),
                    ('meeting_apps', 'zoom;teams;ms-teams;webex;tencentmeeting;wemeet;vlc;potplayer;mpc-hc;mpv'),
                    ('title_exclusions', ''),
                    ('day_start_hour', '0');
                "#,
            )
            .map_err(|error| error.to_string())?;
        // Legacy builds added columns without a schema version. Detect, do not ignore errors.
        for (table, column, declaration) in [
            (
                "web_activity_segments",
                "media_active",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "web_activity_segments",
                "idle",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "activity_segments",
                "idle_exempt",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "web_activity_segments",
                "idle_exempt",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            ("exceptions", "rule_name", "TEXT NOT NULL DEFAULT ''"),
            ("exceptions", "subject_kind", "TEXT NOT NULL DEFAULT ''"),
            ("exceptions", "subject", "TEXT NOT NULL DEFAULT ''"),
            ("exceptions", "focus_id", "INTEGER"),
            ("focus_sessions", "pending_end_at", "INTEGER"),
            (
                "classification_rules",
                "updated_at",
                "INTEGER NOT NULL DEFAULT 0",
            ),
        ] {
            let mut statement = transaction
                .prepare(&format!("PRAGMA table_info({table})"))
                .map_err(|error| error.to_string())?;
            let columns = statement
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            drop(statement);
            if !columns.iter().any(|value| value == column) {
                transaction
                    .execute_batch(&format!(
                        "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
                    ))
                    .map_err(|error| error.to_string())?;
            }
        }
        transaction
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS statistics_defaults (
                target_kind TEXT NOT NULL CHECK(target_kind IN ('application', 'website')),
                target TEXT NOT NULL, category TEXT NOT NULL, updated_at INTEGER NOT NULL,
                PRIMARY KEY(target_kind,target)
             );
             CREATE TABLE IF NOT EXISTS statistics_overrides (
                source_kind TEXT NOT NULL CHECK(source_kind IN ('application', 'website')),
                source_id INTEGER NOT NULL, category TEXT NOT NULL, updated_at INTEGER NOT NULL,
                PRIMARY KEY(source_kind,source_id)
             );
             CREATE TABLE IF NOT EXISTS statistics_exclusions (
                target_kind TEXT NOT NULL CHECK(target_kind IN ('application', 'website')),
                target TEXT NOT NULL, PRIMARY KEY(target_kind,target)
             );
             ",
            )
            .map_err(|error| error.to_string())?;
        focus_engine::migrate(&transaction, version)?;
        web_recording::migrate(&transaction)?;
        limit_reminders::migrate(&transaction)?;
        category_management::migrate(&transaction, version)?;
        category_exchange::validate(&transaction)?;
        timeline_annotations::migrate(&transaction, version)?;
        application_icons::migrate(&transaction, version)?;
        transaction
            .pragma_update(None, "user_version", 9)
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn record_activity(&self, activity: &ActivityIdentity) -> Result<(), String> {
        let now = Utc::now().timestamp_millis();
        let mut activity = activity.clone();
        let settings = self.settings()?;
        let identity = format!("{} {}", activity.app_name, activity.process_path);
        if listed_identity_matches(&settings.title_exclusions, &identity) {
            activity.title.clear();
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        if activity.idle_exempt && activity.idle_seconds > 0 {
            let idle_since = now.saturating_sub(activity.idle_seconds as i64 * 1000);
            transaction
                .execute(
                    "UPDATE activity_segments SET idle_exempt=1
                     WHERE idle=0 AND ended_at>=?1
                       AND app_name=?2 AND process_path=?3",
                    params![idle_since, activity.app_name, activity.process_path],
                )
                .map_err(|error| error.to_string())?;
        }
        if activity.idle && activity.idle_seconds > 0 {
            let idle_since = now.saturating_sub(activity.idle_seconds as i64 * 1000);
            let crossing = transaction
                .query_row(
                    "SELECT id, ended_at, app_name, process_path, title
                     FROM activity_segments
                     WHERE idle=0 AND idle_exempt=0 AND started_at < ?1 AND ended_at > ?1
                     ORDER BY ended_at DESC LIMIT 1",
                    params![idle_since],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| error.to_string())?;
            if let Some((id, previous_end, app_name, process_path, title)) = crossing {
                transaction
                    .execute(
                        "UPDATE activity_segments SET ended_at=?1 WHERE id=?2",
                        params![idle_since, id],
                    )
                    .map_err(|error| error.to_string())?;
                transaction
                    .execute(
                        "INSERT INTO activity_segments
                         (started_at, ended_at, app_name, process_path, title, idle)
                         VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                        params![idle_since, previous_end, app_name, process_path, title],
                    )
                    .map_err(|error| error.to_string())?;
            }
            transaction
                .execute(
                    "UPDATE activity_segments SET idle=1
                     WHERE idle=0 AND idle_exempt=0 AND started_at >= ?1 AND ended_at <= ?2",
                    params![idle_since, now],
                )
                .map_err(|error| error.to_string())?;
        }
        let latest = transaction
            .query_row(
                "SELECT id, ended_at, app_name, process_path, title, idle, idle_exempt
                 FROM activity_segments ORDER BY ended_at DESC, started_at DESC, id DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)? != 0,
                        row.get::<_, i64>(6)? != 0,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;

        if let Some((id, ended_at, app_name, process_path, title, idle, idle_exempt)) = latest {
            let can_merge = now.saturating_sub(ended_at) <= 15_000
                && app_name == activity.app_name
                && process_path == activity.process_path
                && title == activity.title
                && idle == activity.idle
                && idle_exempt == activity.idle_exempt;
            if can_merge {
                transaction
                    .execute(
                        "UPDATE activity_segments SET ended_at = ?1 WHERE id = ?2",
                        params![now, id],
                    )
                    .map_err(|error| error.to_string())?;
                transaction.commit().map_err(|error| error.to_string())?;
                return Ok(());
            }
        }

        transaction
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle, idle_exempt)
                 VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    now,
                    activity.app_name,
                    activity.process_path,
                    activity.title,
                    activity.idle as i64,
                    activity.idle_exempt as i64
                ],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn record_web_activity(&self, activity: &WebActivity) -> Result<(), String> {
        let domain = normalize_domain(&activity.domain);
        if domain.is_empty() {
            return Err("网站域名不能为空".into());
        }
        let settings = self.settings()?;
        let title = if listed_identity_matches(&settings.title_exclusions, &domain) {
            String::new()
        } else {
            activity.title.trim().chars().take(500).collect::<String>()
        };
        let now = Utc::now().timestamp_millis();
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        if activity.idle_exempt && activity.idle_seconds > 0 {
            let idle_since = now.saturating_sub(activity.idle_seconds as i64 * 1000);
            transaction
                .execute(
                    "UPDATE web_activity_segments SET idle_exempt=1
                     WHERE idle=0 AND ended_at>=?1 AND domain=?2",
                    params![idle_since, domain],
                )
                .map_err(|error| error.to_string())?;
        }
        if activity.idle && activity.idle_seconds > 0 {
            let idle_since = now.saturating_sub(activity.idle_seconds as i64 * 1000);
            let crossing = transaction
                .query_row(
                    "SELECT id, ended_at, domain, title, media_active
                     FROM web_activity_segments
                     WHERE idle=0 AND idle_exempt=0 AND started_at < ?1 AND ended_at > ?1
                     ORDER BY ended_at DESC LIMIT 1",
                    params![idle_since],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| error.to_string())?;
            if let Some((id, previous_end, domain, previous_title, media_active)) = crossing {
                transaction
                    .execute(
                        "UPDATE web_activity_segments SET ended_at=?1 WHERE id=?2",
                        params![idle_since, id],
                    )
                    .map_err(|error| error.to_string())?;
                transaction
                    .execute(
                        "INSERT INTO web_activity_segments
                         (started_at, ended_at, domain, title, media_active, idle)
                         VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                        params![
                            idle_since,
                            previous_end,
                            domain,
                            previous_title,
                            media_active
                        ],
                    )
                    .map_err(|error| error.to_string())?;
            }
            transaction
                .execute(
                    "UPDATE web_activity_segments SET idle=1
                     WHERE idle=0 AND idle_exempt=0 AND started_at >= ?1 AND ended_at <= ?2",
                    params![idle_since, now],
                )
                .map_err(|error| error.to_string())?;
        }
        let latest = transaction
            .query_row(
                "SELECT id, ended_at, domain, title, media_active, idle, idle_exempt
                 FROM web_activity_segments ORDER BY ended_at DESC, started_at DESC, id DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)? != 0,
                        row.get::<_, i64>(5)? != 0,
                        row.get::<_, i64>(6)? != 0,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some((
            id,
            ended_at,
            previous_domain,
            previous_title,
            previous_media_active,
            previous_idle,
            previous_idle_exempt,
        )) = latest
        {
            if now.saturating_sub(ended_at) <= 15_000
                && previous_domain == domain
                && previous_title == title
                && previous_media_active == activity.media_active
                && previous_idle == activity.idle
                && previous_idle_exempt == activity.idle_exempt
            {
                transaction
                    .execute(
                        "UPDATE web_activity_segments SET ended_at = ?1 WHERE id = ?2",
                        params![now, id],
                    )
                    .map_err(|error| error.to_string())?;
                transaction.commit().map_err(|error| error.to_string())?;
                return Ok(());
            }
        }
        transaction
            .execute(
                "INSERT INTO web_activity_segments
                 (started_at, ended_at, domain, title, media_active, idle, idle_exempt)
                 VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    now,
                    domain,
                    title,
                    activity.media_active as i64,
                    activity.idle as i64,
                    activity.idle_exempt as i64
                ],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn should_count_idle_as_active(&self, activity: &ActivityIdentity) -> Result<bool, String> {
        let settings = self.settings()?;
        let identity = format!("{} {}", activity.app_name, activity.process_path).to_lowercase();
        if group_names(&settings.meeting_apps).any(|name| identity.contains(&name.to_lowercase())) {
            return Ok(true);
        }
        if settings.count_media_while_idle && identity.contains("msedge") {
            if let Some(confirmed) = self.confirmed_website_media(activity)? {
                return Ok(confirmed);
            }
            let recent_media = self
                .connection()?
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM web_activity_segments
                     WHERE media_active = 1 AND ended_at >= ?1)",
                    params![Utc::now().timestamp_millis() - 15_000],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| error.to_string())?
                != 0;
            return Ok(recent_media);
        }
        Ok(false)
    }

    pub fn settings(&self) -> Result<AppSettings, String> {
        let connection = self.connection()?;
        let value = |key: &str, fallback: &str| -> Result<String, String> {
            connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key = ?1",
                    params![key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|error| error.to_string())
                .map(|value| value.unwrap_or_else(|| fallback.to_string()))
        };
        Ok(AppSettings {
            idle_minutes: value("idle_minutes", "5")?.parse().unwrap_or(5),
            count_media_while_idle: value("count_media_while_idle", "true")? == "true",
            meeting_apps: value("meeting_apps", "")?,
            title_exclusions: value("title_exclusions", "")?,
            day_start_hour: value("day_start_hour", "0")?.parse().unwrap_or(0),
        })
    }

    pub fn update_settings(&self, settings: AppSettings) -> Result<AppSettings, String> {
        if !(1..=60).contains(&settings.idle_minutes) {
            return Err("闲置阈值必须在 1 到 60 分钟之间".into());
        }
        if !(0..=23).contains(&settings.day_start_hour) {
            return Err("每日换日时间必须在 0 到 23 点之间".into());
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        for (key, value) in [
            ("idle_minutes", settings.idle_minutes.to_string()),
            (
                "count_media_while_idle",
                settings.count_media_while_idle.to_string(),
            ),
            ("meeting_apps", settings.meeting_apps.trim().to_string()),
            (
                "title_exclusions",
                settings.title_exclusions.trim().to_string(),
            ),
            ("day_start_hour", settings.day_start_hour.to_string()),
        ] {
            transaction
                .execute(
                    "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        self.settings()
    }

    fn today_start_millis(&self) -> i64 {
        let local = Local::now();
        let day_start_hour = self
            .settings()
            .map(|settings| settings.day_start_hour.clamp(0, 23) as u32)
            .unwrap_or(0);
        let date = if local.hour() < day_start_hour {
            local.date_naive() - chrono::Duration::days(1)
        } else {
            local.date_naive()
        };
        let naive = date
            .and_hms_opt(day_start_hour, 0, 0)
            .expect("valid midnight");
        Local
            .from_local_datetime(&naive)
            .earliest()
            .unwrap_or(local)
            .with_timezone(&Utc)
            .timestamp_millis()
    }

    fn has_active_exception(
        connection: &Connection,
        kind: &str,
        rule_id: i64,
        now: i64,
        focus_id: Option<i64>,
        subject_kind: &str,
        subject: &str,
    ) -> Result<bool, String> {
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM exceptions
                 WHERE kind=?1 AND (target=?2 OR ?1='focus') AND started_at<=?3 AND ended_at>?3
                   AND (?1!='focus' OR focus_id=?4)
                   AND ((subject='' AND (?1!='focus' OR target=?2))
                        OR (subject_kind=?5 AND (lower(subject)=lower(?6)
                            OR (?5='application' AND ?6='steam' AND lower(subject) IN ('steam.exe','steamwebhelper','steamwebhelper.exe'))))))",
                params![
                    kind,
                    rule_id.to_string(),
                    now,
                    focus_id,
                    subject_kind,
                    subject
                ],
                |row| row.get::<_, i64>(0),
            )
            .map(|exists| exists != 0)
            .map_err(|error| error.to_string())
    }

    pub fn statistics(&self, start: i64, end: i64) -> Result<ActivityStatistics, String> {
        let span = end.saturating_sub(start);
        let bucket = if span <= 2 * 86_400_000 {
            "hour"
        } else if span <= 90 * 86_400_000 {
            "day"
        } else {
            "month"
        };
        self.query_statistics(crate::domain::StatisticsQuery {
            start,
            end,
            bucket: bucket.into(),
            target_kind: None,
            target: None,
            offset: None,
            limit: None,
            search: None,
            include_excluded: None,
            page_key: None,
        })
    }

    pub fn dashboard(&self) -> Result<Dashboard, String> {
        self.dashboard_with_statistics(true)
    }

    pub fn dashboard_with_statistics(&self, include_statistics: bool) -> Result<Dashboard, String> {
        let start = self.today_start_millis();
        let now = Utc::now().timestamp_millis();
        let current_activity = self.connection()?.query_row(
            "SELECT app_name, process_path, title, idle, idle_exempt FROM activity_segments ORDER BY ended_at DESC, started_at DESC, id DESC LIMIT 1", [],
            |row| Ok(ActivityIdentity {
                app_name: row.get(0)?, process_path: row.get(1)?, title: row.get(2)?,
                idle: row.get::<_, i64>(3)? != 0, idle_seconds: 0,
                idle_exempt: row.get::<_, i64>(4)? != 0,
            }),
        ).optional().map_err(|error| error.to_string())?;
        let mut dashboard = Dashboard {
            generated_at: now,
            day_started_at: start,
            effective_seconds: 0,
            tracked_seconds: 0,
            effective_milliseconds: 0,
            tracked_milliseconds: 0,
            category_usage: Vec::new(),
            trend: Vec::new(),
            current_activity,
            app_usage: Vec::new(),
            site_usage: Vec::new(),
            timeline: Vec::new(),
            focus: self.active_focus()?,
        };
        if include_statistics {
            let stats = self.statistics(start, now.max(start + 1))?;
            dashboard.effective_seconds = stats.effective_seconds;
            dashboard.tracked_seconds = stats.tracked_seconds;
            dashboard.effective_milliseconds = stats.effective_milliseconds;
            dashboard.tracked_milliseconds = stats.tracked_milliseconds;
            dashboard.category_usage = stats.category_usage;
            dashboard.trend = stats.trend;
            dashboard.app_usage = stats.app_usage;
            dashboard.site_usage = stats.site_usage;
            dashboard.timeline = stats.timeline;
        }
        Ok(dashboard)
    }

    pub fn active_focus(&self) -> Result<Option<FocusState>, String> {
        Ok(self
            .latest_focus()?
            .filter(|state| matches!(state.status.as_str(), "focusing" | "away")))
    }

    pub fn start_focus(
        &self,
        label: String,
        minutes: i64,
        mode: String,
    ) -> Result<FocusState, String> {
        self.start_ad_hoc_focus(label, minutes, mode)
    }

    pub fn start_focus_leave(&self) -> Result<FocusState, String> {
        let focus = self
            .active_focus()?
            .ok_or_else(|| "当前没有进行中的专注".to_string())?;
        self.pause_focus(focus.id)
    }

    pub fn return_to_focus(&self) -> Result<FocusState, String> {
        let focus = self
            .active_focus()?
            .ok_or_else(|| "当前没有进行中的专注".to_string())?;
        self.resume_focus(focus.id)
    }

    pub fn end_focus(&self) -> Result<FocusState, String> {
        let focus = self
            .active_focus()?
            .ok_or_else(|| "当前没有进行中的专注".to_string())?;
        self.complete_focus(focus.id)
    }

    pub fn cancel_focus_end(&self) -> Result<FocusState, String> {
        let focus = self
            .active_focus()?
            .ok_or_else(|| "当前没有进行中的专注".to_string())?;
        self.connection()?
            .execute(
                "UPDATE focus_sessions SET pending_end_at = NULL WHERE id = ?1",
                params![focus.id],
            )
            .map_err(|error| error.to_string())?;
        self.active_focus()?.ok_or_else(|| "专注状态已结束".into())
    }

    pub fn record_exception(
        &self,
        kind: String,
        target: String,
        subject_kind: String,
        subject: String,
        minutes: i64,
        reason: String,
    ) -> Result<(), String> {
        if !(1..=180).contains(&minutes) {
            return Err("加时时长必须在 1 到 180 分钟之间".into());
        }
        if kind != "daily" && kind != "focus" {
            return Err("未知的紧急放行类型".into());
        }
        let focus_id = if kind == "focus" {
            Some(
                self.active_focus()?
                    .ok_or_else(|| "当前没有进行中的专注".to_string())?
                    .id,
            )
        } else {
            None
        };
        let rule = self
            .connection()?
            .query_row(
                "SELECT name, target_kind, target_pattern FROM limit_rules WHERE id = ?1",
                params![target.parse::<i64>().unwrap_or_default()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if kind == "daily" && rule.is_none() {
            return Err("指定的限制规则不存在".into());
        }
        let (rule_name, target_kind, pattern) =
            rule.unwrap_or_else(|| ("专注白名单".into(), String::new(), String::new()));
        // Legacy callers supplied only a non-group rule. Infer its exact subject
        // once; new focus exits do not require a daily rule to exist.
        let (subject_kind, subject) = if kind == "focus"
            && subject.trim().is_empty()
            && matches!(target_kind.as_str(), "application" | "website")
            && !pattern.contains([';', ','])
        {
            (target_kind.clone(), pattern)
        } else {
            (subject_kind.trim().to_lowercase(), subject)
        };
        let subject = match subject_kind.as_str() {
            "application" => {
                normalize_application_subject(&crate::app_identity::canonical_executable(&subject))
            }
            "website" => normalize_domain(&subject),
            _ => subject.trim().to_lowercase(),
        };
        if target_kind == "group"
            && (!matches!(subject_kind.as_str(), "application" | "website") || subject.is_empty())
        {
            return Err("组规则放行需要指定当前应用或网站".into());
        }
        if !subject.is_empty() && !matches!(subject_kind.as_str(), "application" | "website") {
            return Err("放行目标类型无效".into());
        }
        if kind == "focus" && subject.is_empty() {
            return Err("专注紧急出口需要指定应用或网站".into());
        }
        let start = Utc::now().timestamp_millis();
        self.connection()?.execute(
            "INSERT INTO exceptions
             (kind, target, started_at, ended_at, reason, rule_name, subject_kind, subject, focus_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![kind, target, start, start + minutes * 60_000, reason, rule_name, subject_kind, subject, focus_id],
        ).map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn exception_history(&self, limit: i64) -> Result<Vec<ExceptionUsage>, String> {
        let limit = limit.clamp(1, 100);
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, kind, target, started_at, ended_at, reason, rule_name,
                        subject_kind, subject
                 FROM exceptions ORDER BY started_at DESC LIMIT ?1",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        drop(statement);
        let rules = self.list_rules()?;
        let now = Utc::now().timestamp_millis();
        rows.into_iter()
            .map(
                |(
                    id,
                    kind,
                    target,
                    started_at,
                    ended_at,
                    reason,
                    stored_name,
                    subject_kind,
                    subject,
                )| {
                    let rule = target
                        .parse::<i64>()
                        .ok()
                        .and_then(|target_id| rules.iter().find(|rule| rule.id == target_id));
                    let actual_end = ended_at.min(now);
                    let actual_seconds = if let Some(rule) = rule {
                        if rule.target_kind == "group" && subject.is_empty() {
                            Self::group_usage_seconds(
                                &connection,
                                started_at,
                                actual_end,
                                rule,
                                &rules,
                            )?
                        } else {
                            let category_rule = ClassificationRule {
                                id: rule.id,
                                name: rule.name.clone(),
                                target_kind: if subject.is_empty() {
                                    rule.target_kind.clone()
                                } else {
                                    subject_kind.clone()
                                },
                                target_pattern: if subject.is_empty() {
                                    rule.target_pattern.clone()
                                } else {
                                    subject.clone()
                                },
                                category: String::new(),
                                enabled: true,
                            };
                            merged_seconds(Self::classification_intervals(
                                &connection,
                                started_at,
                                actual_end,
                                &category_rule,
                            )?)
                        }
                    } else {
                        0
                    };
                    Ok(ExceptionUsage {
                        id,
                        kind,
                        rule_name: rule
                            .map(|rule| rule.name.clone())
                            .filter(|name| !name.is_empty())
                            .unwrap_or(stored_name),
                        subject,
                        started_at,
                        ended_at,
                        granted_seconds: ended_at.saturating_sub(started_at) / 1000,
                        actual_seconds,
                        reason,
                    })
                },
            )
            .collect()
    }

    pub fn end_exception(&self, id: i64) -> Result<(), String> {
        let changed = self
            .connection()?
            .execute(
                "UPDATE exceptions SET ended_at=?1 WHERE id=?2 AND ended_at>?1",
                params![Utc::now().timestamp_millis(), id],
            )
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err("这次紧急放行已经结束或不存在".into());
        }
        Ok(())
    }

    fn application_intervals(
        connection: &Connection,
        start: i64,
        end: i64,
        pattern: &str,
    ) -> Result<Vec<(i64, i64)>, String> {
        let candidate = if pattern.contains([';', ',']) {
            "%".to_string()
        } else {
            let key = crate::app_identity::canonical_executable(pattern);
            let stem = if key == "steam.exe" {
                "steam"
            } else {
                pattern.trim().trim_end_matches(".exe")
            };
            format!("%{}%", stem.to_lowercase())
        };
        let mut query = connection.prepare("SELECT MAX(started_at,?1),MIN(ended_at,?2),process_path,app_name FROM activity_segments WHERE ended_at>?1 AND started_at<?2 AND idle=0 AND (lower(process_path) LIKE ?3 OR lower(app_name) LIKE ?3)").map_err(|e|e.to_string())?;
        let rows = query
            .query_map(params![start, end, candidate], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut intervals = Vec::new();
        for row in rows {
            let (start, end, path, name) = row.map_err(|e| e.to_string())?;
            if end > start && crate::app_identity::matches(&path, &name, pattern) {
                intervals.push((start, end));
            }
        }
        Ok(intervals)
    }

    fn classification_intervals(
        connection: &Connection,
        start: i64,
        end: i64,
        rule: &ClassificationRule,
    ) -> Result<Vec<(i64, i64)>, String> {
        let pattern = rule.target_pattern.trim().to_lowercase();
        if pattern.is_empty() {
            return Ok(Vec::new());
        }
        if rule.target_kind == "application" {
            return Self::application_intervals(connection, start, end, &pattern);
        }
        if rule.target_kind == "website" {
            let domain = normalize_domain(&pattern);
            let subdomain_pattern = format!("%.{domain}");
            let mut statement = connection
                .prepare(
                    "SELECT MAX(started_at, ?1), MIN(ended_at, ?2)
                     FROM web_activity_segments
                     WHERE ended_at >= ?1 AND started_at <= ?2 AND idle = 0
                       AND (domain = ?3 OR domain LIKE ?4)",
                )
                .map_err(|error| error.to_string())?;
            return statement
                .query_map(params![start, end, domain, subdomain_pattern], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string());
        }
        if rule.target_kind == "title" {
            let search = format!("%{pattern}%");
            let mut statement = connection
                .prepare(
                    "SELECT MAX(started_at, ?1), MIN(ended_at, ?2) FROM (
                         SELECT started_at, ended_at FROM activity_segments
                         WHERE ended_at >= ?1 AND started_at <= ?2 AND idle = 0
                           AND lower(title) LIKE ?3
                         UNION ALL
                         SELECT started_at, ended_at FROM web_activity_segments
                         WHERE ended_at >= ?1 AND started_at <= ?2 AND idle = 0
                           AND lower(title) LIKE ?3
                     )",
                )
                .map_err(|error| error.to_string())?;
            return statement
                .query_map(params![start, end, search], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string());
        }
        Ok(Vec::new())
    }

    fn group_usage_seconds(
        connection: &Connection,
        start: i64,
        end: i64,
        group: &LimitRule,
        rules: &[LimitRule],
    ) -> Result<i64, String> {
        let mut intervals = Vec::new();
        for member in rules.iter().filter(|rule| {
            rule.enabled && rule.target_kind != "group" && rule_belongs_to_group(rule, group)
        }) {
            if member.target_kind == "application" {
                intervals.extend(Self::application_intervals(
                    connection,
                    start,
                    end,
                    &member.target_pattern,
                )?);
            } else if member.target_kind == "website" {
                let pattern = normalize_domain(&member.target_pattern);
                let subdomain_pattern = format!("%.{pattern}");
                let mut statement = connection
                    .prepare(
                        "SELECT MAX(started_at, ?1), MIN(ended_at, ?2)
                         FROM web_activity_segments
                         WHERE ended_at >= ?1 AND started_at <= ?2 AND idle = 0
                           AND (domain = ?3 OR domain LIKE ?4)",
                    )
                    .map_err(|error| error.to_string())?;
                intervals.extend(
                    statement
                        .query_map(params![start, end, pattern, subdomain_pattern], |row| {
                            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                        })
                        .map_err(|error| error.to_string())?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())?,
                );
            }
        }
        Ok(merged_seconds(intervals))
    }

    pub fn list_rules(&self) -> Result<Vec<LimitRule>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, name, target_kind, target_pattern, group_name,
                    daily_limit_minutes, enabled FROM limit_rules ORDER BY enabled DESC, name",
            )
            .map_err(|error| error.to_string())?;
        let mut rules = statement
            .query_map([], |row| {
                Ok(LimitRule {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    target_kind: row.get(2)?,
                    target_pattern: row.get(3)?,
                    group_name: row.get(4)?,
                    daily_limit_minutes: row.get(5)?,
                    enabled: row.get::<_, i64>(6)? != 0,
                    used_seconds: 0,
                    remaining_seconds: row.get::<_, i64>(5)? * 60,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let today = self.today_start_millis();
        let now = Utc::now().timestamp_millis();
        for rule in &mut rules {
            if rule.target_kind == "application" {
                rule.used_seconds = merged_seconds(Self::application_intervals(
                    &connection,
                    today,
                    now,
                    &rule.target_pattern,
                )?);
            } else if rule.target_kind == "website" {
                let pattern = normalize_domain(&rule.target_pattern);
                let subdomain_pattern = format!("%.{pattern}");
                rule.used_seconds = connection
                    .query_row(
                        "SELECT COALESCE(SUM(MAX(0, MIN(ended_at, ?2) - MAX(started_at, ?1))) / 1000, 0)
                         FROM web_activity_segments
                         WHERE ended_at >= ?1 AND started_at <= ?2 AND idle = 0
                           AND (domain = ?3 OR domain LIKE ?4)",
                        params![today, now, pattern, subdomain_pattern],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
            } else {
                continue;
            }
            rule.remaining_seconds =
                (rule.daily_limit_minutes * 60).saturating_sub(rule.used_seconds);
        }
        let snapshot = rules.clone();
        for rule in &mut rules {
            if rule.target_kind == "group" {
                rule.used_seconds =
                    Self::group_usage_seconds(&connection, today, now, rule, &snapshot)?;
                rule.remaining_seconds =
                    (rule.daily_limit_minutes * 60).saturating_sub(rule.used_seconds);
            }
        }
        Ok(rules)
    }

    pub fn create_rule(&self, rule: NewLimitRule) -> Result<LimitRule, String> {
        if rule.name.trim().is_empty() || rule.target_pattern.trim().is_empty() {
            return Err("名称和匹配目标不能为空".into());
        }
        if !matches!(
            rule.target_kind.as_str(),
            "application" | "website" | "group"
        ) {
            return Err("未知的限制目标类型".into());
        }
        if !(1..=1440).contains(&rule.daily_limit_minutes) {
            return Err("每日额度必须在 1 到 1440 分钟之间".into());
        }
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO limit_rules (name, target_kind, target_pattern, group_name, daily_limit_minutes)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![rule.name.trim(), rule.target_kind, rule.target_pattern.trim(), rule.group_name.trim(), rule.daily_limit_minutes],
        ).map_err(|error| error.to_string())?;
        let id = connection.last_insert_rowid();
        self.list_rules()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| "无法读取刚创建的规则".into())
    }

    pub fn update_rule(&self, id: i64, rule: NewLimitRule) -> Result<LimitRule, String> {
        if rule.name.trim().is_empty() || rule.target_pattern.trim().is_empty() {
            return Err("名称和匹配目标不能为空".into());
        }
        if !matches!(
            rule.target_kind.as_str(),
            "application" | "website" | "group"
        ) {
            return Err("未知的限制目标类型".into());
        }
        if !(1..=1440).contains(&rule.daily_limit_minutes) {
            return Err("每日额度必须在 1 到 1440 分钟之间".into());
        }
        let connection = self.connection()?;
        let enabled = connection
            .query_row(
                "SELECT enabled FROM limit_rules WHERE id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "限制规则不存在".to_string())?
            != 0;
        if enabled && self.active_focus()?.is_some() {
            return Err("专注进行中，请先关闭这条规则并等待倒计时结束后再编辑".into());
        }
        connection
            .execute(
                "UPDATE limit_rules SET name=?1, target_kind=?2, target_pattern=?3,
                 group_name=?4, daily_limit_minutes=?5 WHERE id=?6",
                params![
                    rule.name.trim(),
                    rule.target_kind,
                    rule.target_pattern.trim(),
                    rule.group_name.trim(),
                    rule.daily_limit_minutes,
                    id
                ],
            )
            .map_err(|error| error.to_string())?;
        self.list_rules()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| "限制规则不存在".into())
    }

    pub fn delete_rule(&self, id: i64) -> Result<(), String> {
        let connection = self.connection()?;
        let enabled = connection
            .query_row(
                "SELECT enabled FROM limit_rules WHERE id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "限制规则不存在".to_string())?
            != 0;
        if enabled && self.active_focus()?.is_some() {
            return Err("专注进行中，请先关闭这条规则并等待倒计时结束后再删除".into());
        }
        connection
            .execute("DELETE FROM limit_rules WHERE id = ?1", params![id])
            .map_err(|error| error.to_string())?;
        connection
            .execute(
                "DELETE FROM pending_rule_changes WHERE rule_id = ?1",
                params![id],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn list_classification_rules(&self) -> Result<Vec<ClassificationRule>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, name, target_kind, target_pattern, category, enabled
                 FROM classification_rules ORDER BY enabled DESC, category, name",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok(ClassificationRule {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    target_kind: row.get(2)?,
                    target_pattern: row.get(3)?,
                    category: row.get(4)?,
                    enabled: row.get::<_, i64>(5)? != 0,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(rows)
    }

    fn validate_classification(rule: &NewClassificationRule) -> Result<(), String> {
        if rule.name.trim().is_empty()
            || rule.target_pattern.trim().is_empty()
            || rule.category.trim().is_empty()
        {
            return Err("名称、匹配目标和分类不能为空".into());
        }
        if !matches!(
            rule.target_kind.as_str(),
            "application" | "website" | "title"
        ) {
            return Err("未知的分类匹配类型".into());
        }
        Ok(())
    }

    pub fn create_classification_rule(
        &self,
        rule: NewClassificationRule,
    ) -> Result<ClassificationRule, String> {
        Self::validate_classification(&rule)?;
        let mut database = self.connection()?;
        let connection = database
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        connection
            .execute(
                "INSERT INTO classification_rules (name, target_kind, target_pattern, category, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    rule.name.trim(),
                    rule.target_kind,
                    rule.target_pattern.trim(),
                    rule.category.trim(),
                    category_management::next_revision(&connection)?
                ],
            )
            .map_err(|error| error.to_string())?;
        let id = connection.last_insert_rowid();
        connection.commit().map_err(|e| e.to_string())?;
        self.list_classification_rules()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| "无法读取刚创建的分类规则".into())
    }

    pub fn update_classification_rule(
        &self,
        id: i64,
        rule: NewClassificationRule,
    ) -> Result<ClassificationRule, String> {
        Self::validate_classification(&rule)?;
        let mut database = self.connection()?;
        let connection = database
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let changed = connection
            .execute(
                "UPDATE classification_rules SET name=?1, target_kind=?2,
                 target_pattern=?3, category=?4, updated_at=?6 WHERE id=?5",
                params![
                    rule.name.trim(),
                    rule.target_kind,
                    rule.target_pattern.trim(),
                    rule.category.trim(),
                    id,
                    category_management::next_revision(&connection)?
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err("分类规则不存在".into());
        }
        connection.commit().map_err(|e| e.to_string())?;
        self.list_classification_rules()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| "分类规则不存在".into())
    }

    pub fn set_classification_rule_enabled(&self, id: i64, enabled: bool) -> Result<(), String> {
        let mut database = self.connection()?;
        let connection = database
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let changed = connection
            .execute(
                "UPDATE classification_rules SET enabled=?1, updated_at=?3 WHERE id=?2",
                params![
                    enabled as i64,
                    id,
                    category_management::next_revision(&connection)?
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err("分类规则不存在".into());
        }
        connection.commit().map_err(|e| e.to_string())
    }

    pub fn delete_classification_rule(&self, id: i64) -> Result<(), String> {
        let changed = self
            .connection()?
            .execute("DELETE FROM classification_rules WHERE id=?1", params![id])
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err("分类规则不存在".into());
        }
        Ok(())
    }

    pub fn set_rule_enabled(&self, id: i64, enabled: bool) -> Result<RuleChangeResult, String> {
        let connection = self.connection()?;
        let exists = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM limit_rules WHERE id = ?1)",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| error.to_string())?
            != 0;
        if !exists {
            return Err("限制规则不存在".into());
        }
        connection
            .execute(
                "DELETE FROM pending_rule_changes WHERE rule_id = ?1",
                params![id],
            )
            .map_err(|error| error.to_string())?;
        if enabled || self.active_focus()?.is_none() {
            connection
                .execute(
                    "UPDATE limit_rules SET enabled = ?1 WHERE id = ?2",
                    params![enabled as i64, id],
                )
                .map_err(|error| error.to_string())?;
            return Ok(RuleChangeResult {
                pending: false,
                apply_at: None,
            });
        }
        let apply_at = Utc::now().timestamp_millis() + 30_000;
        connection
            .execute(
                "INSERT INTO pending_rule_changes (rule_id, enabled, apply_at, created_at)
                 VALUES (?1, 0, ?2, ?3)",
                params![id, apply_at, Utc::now().timestamp_millis()],
            )
            .map_err(|error| error.to_string())?;
        Ok(RuleChangeResult {
            pending: true,
            apply_at: Some(apply_at),
        })
    }

    pub fn apply_pending_rule_changes(&self) -> Result<usize, String> {
        let now = Utc::now().timestamp_millis();
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let pending = {
            let mut statement = transaction
                .prepare(
                    "SELECT id, rule_id, enabled FROM pending_rule_changes
                     WHERE apply_at <= ?1 ORDER BY id",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map(params![now], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            rows
        };
        for (change_id, rule_id, enabled) in &pending {
            transaction
                .execute(
                    "UPDATE limit_rules SET enabled = ?1 WHERE id = ?2",
                    params![enabled, rule_id],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "DELETE FROM pending_rule_changes WHERE id = ?1",
                    params![change_id],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(pending.len())
    }

    pub fn restriction_for(
        &self,
        activity: &ActivityIdentity,
    ) -> Result<Option<RestrictionNotice>, String> {
        if activity.idle {
            return Ok(None);
        }
        let now = Utc::now().timestamp_millis();
        let focus = self.active_focus()?;
        let focus_is_blocking = focus
            .as_ref()
            .is_some_and(|session| session.active_leave_started_at.is_none());
        let connection = self.connection()?;
        let mut causes = Vec::new();
        let rules = self.list_rules()?;
        let subject = normalize_application_subject(&crate::app_identity::canonical_executable(
            if activity.process_path.is_empty() {
                &activity.app_name
            } else {
                &activity.process_path
            },
        ));
        let mut focus_exception = Self::has_active_exception(
            &connection,
            "focus",
            0,
            now,
            focus.as_ref().map(|session| session.id),
            "application",
            &subject,
        )?;

        for rule in rules.iter().filter(|rule| rule.enabled) {
            let applies = if rule.target_kind == "application" {
                application_matches_rule(activity, rule)
            } else if rule.target_kind == "group" {
                rules.iter().any(|member| {
                    member.enabled
                        && member.target_kind == "application"
                        && rule_belongs_to_group(member, rule)
                        && application_matches_rule(activity, member)
                })
            } else {
                false
            };
            if !applies {
                continue;
            }
            let used_seconds = rule.used_seconds;
            let daily_exception = Self::has_active_exception(
                &connection,
                "daily",
                rule.id,
                now,
                None,
                "application",
                &subject,
            )?;
            let limit_seconds = rule.daily_limit_minutes * 60;
            if used_seconds >= limit_seconds && !daily_exception {
                causes.push(RestrictionCause {
                    rule_id: rule.id,
                    rule_name: rule.name.clone(),
                    kind: "daily".into(),
                    used_seconds,
                    limit_seconds,
                });
            }

            focus_exception |= Self::has_active_exception(
                &connection,
                "focus",
                rule.id,
                now,
                focus.as_ref().map(|session| session.id),
                "application",
                &subject,
            )?;
        }
        if focus_is_blocking
            && !focus_exception
            && !self.focus_allowed("application", &subject, now)?
        {
            causes.push(RestrictionCause {
                rule_id: 0,
                rule_name: focus.as_ref().map(|s| s.label.clone()).unwrap_or_default(),
                kind: "focus".into(),
                used_seconds: 0,
                limit_seconds: 0,
            });
        }
        if causes.is_empty() {
            Ok(None)
        } else {
            Ok(Some(RestrictionNotice {
                app_name: crate::app_identity::display_name(&subject, &activity.app_name),
                title: activity.title.clone(),
                causes,
                subject_kind: "application".into(),
                subject,
            }))
        }
    }

    #[cfg(test)]
    pub fn web_restriction_for(
        &self,
        activity: &WebActivity,
    ) -> Result<Option<RestrictionNotice>, String> {
        if activity.idle {
            return Ok(None);
        }
        let now = Utc::now().timestamp_millis();
        let connection = self.connection()?;
        let mut causes = Vec::new();
        let rules = self.list_rules()?;
        let domain = normalize_domain(&activity.domain);
        // This release applies focus protection to applications only. Existing
        // website statistics and daily quotas remain independent and unchanged.
        for rule in rules.iter().filter(|rule| rule.enabled) {
            let applies = if rule.target_kind == "website" {
                domain_matches(&activity.domain, &rule.target_pattern)
            } else if rule.target_kind == "group" {
                rules.iter().any(|member| {
                    member.enabled
                        && member.target_kind == "website"
                        && rule_belongs_to_group(member, rule)
                        && domain_matches(&activity.domain, &member.target_pattern)
                })
            } else {
                false
            };
            if !applies {
                continue;
            }
            let daily_exception = Self::has_active_exception(
                &connection,
                "daily",
                rule.id,
                now,
                None,
                "website",
                &domain,
            )?;
            let limit_seconds = rule.daily_limit_minutes * 60;
            if rule.used_seconds >= limit_seconds && !daily_exception {
                causes.push(RestrictionCause {
                    rule_id: rule.id,
                    rule_name: rule.name.clone(),
                    kind: "daily".into(),
                    used_seconds: rule.used_seconds,
                    limit_seconds,
                });
            }
        }
        if causes.is_empty() {
            Ok(None)
        } else {
            Ok(Some(RestrictionNotice {
                app_name: normalize_domain(&activity.domain),
                title: activity.title.clone(),
                causes,
                subject_kind: "website".into(),
                subject: domain,
            }))
        }
    }

    pub fn import_schedule(
        &self,
        date: String,
        schedule: ScheduleImport,
    ) -> Result<Vec<ScheduleItem>, String> {
        NaiveDate::parse_from_str(&date, "%Y-%m-%d")
            .map_err(|_| "日期格式必须是 YYYY-MM-DD".to_string())?;
        if schedule.name.trim().is_empty() {
            return Err("日程名称不能为空".into());
        }
        if schedule.items.is_empty() || schedule.items.len() > 48 {
            return Err("日程必须包含 1 到 48 个时段".into());
        }
        let mut validated = Vec::with_capacity(schedule.items.len());
        for item in schedule.items {
            if item.label.trim().is_empty() {
                return Err("时段名称不能为空".into());
            }
            let time = NaiveTime::parse_from_str(&item.start, "%H:%M")
                .map_err(|_| format!("无效开始时间：{}", item.start))?;
            if !(1..=720).contains(&item.duration_minutes) {
                return Err("时段长度必须在 1 到 720 分钟之间".into());
            }
            if item.mode != "normal" && item.mode != "strict" {
                return Err("日程模式只能是 normal 或 strict".into());
            }
            validated.push((
                item.label.trim().to_string(),
                time.hour() as i64 * 60 + time.minute() as i64,
                item.duration_minutes,
                item.mode,
                item.task_id,
            ));
        }
        validated.sort_unstable_by_key(|item| item.1);
        if validated
            .windows(2)
            .any(|pair| pair[1].1 < pair[0].1 + pair[0].2)
        {
            return Err("日程时段不能互相重叠".into());
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "DELETE FROM scheduled_focus WHERE date = ?1 AND started_focus_id IS NULL",
                params![date],
            )
            .map_err(|error| error.to_string())?;
        for (label, start_minute, duration, mode, task_id) in validated {
            if let Some(task_id) = task_id {
                let exists: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM focus_tasks WHERE id=?1 AND deleted_at IS NULL)",
                    params![task_id], |row| row.get(0)).map_err(|e|e.to_string())?;
                if !exists {
                    return Err("关联的任务不存在，请重新选择".into());
                }
            }
            transaction
                .execute(
                    "INSERT INTO scheduled_focus
                     (date, source_name, label, start_minute, duration_minutes, mode, task_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        date,
                        schedule.name.trim(),
                        label,
                        start_minute,
                        duration,
                        mode,
                        task_id
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        self.list_schedule(date)
    }

    pub fn list_schedule(&self, date: String) -> Result<Vec<ScheduleItem>, String> {
        NaiveDate::parse_from_str(&date, "%Y-%m-%d")
            .map_err(|_| "日期格式必须是 YYYY-MM-DD".to_string())?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, date, source_name, label, start_minute, duration_minutes,
                        mode, enabled, started_focus_id, task_id
                 FROM scheduled_focus WHERE date = ?1 ORDER BY start_minute, id",
            )
            .map_err(|error| error.to_string())?;
        let items = statement
            .query_map(params![date], |row| {
                let minute = row.get::<_, i64>(4)?;
                Ok(ScheduleItem {
                    id: row.get(0)?,
                    date: row.get(1)?,
                    source_name: row.get(2)?,
                    label: row.get(3)?,
                    start: format!("{:02}:{:02}", minute / 60, minute % 60),
                    duration_minutes: row.get(5)?,
                    mode: row.get(6)?,
                    enabled: row.get::<_, i64>(7)? != 0,
                    started: row.get::<_, Option<i64>>(8)?.is_some(),
                    task_id: row.get(9)?,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(items)
    }

    pub fn save_schedule_item(
        &self,
        id: Option<i64>,
        date: String,
        item: ScheduleImportItem,
    ) -> Result<ScheduleItem, String> {
        NaiveDate::parse_from_str(&date, "%Y-%m-%d")
            .map_err(|_| "日期格式必须是 YYYY-MM-DD".to_string())?;
        if item.label.trim().is_empty() {
            return Err("时段名称不能为空".into());
        }
        let time = NaiveTime::parse_from_str(&item.start, "%H:%M")
            .map_err(|_| "开始时间格式必须是 HH:MM".to_string())?;
        if !(1..=720).contains(&item.duration_minutes) {
            return Err("时段长度必须在 1 到 720 分钟之间".into());
        }
        if item.mode != "normal" && item.mode != "strict" {
            return Err("日程模式只能是 normal 或 strict".into());
        }
        let start_minute = time.hour() as i64 * 60 + time.minute() as i64;
        let connection = self.connection()?;
        if let Some(task_id) = item.task_id {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM focus_tasks WHERE id=?1 AND deleted_at IS NULL)",
                    params![task_id],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !exists {
                return Err("关联的任务不存在，请重新选择".into());
            }
        }
        let overlaps = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scheduled_focus
                 WHERE date=?1 AND id != COALESCE(?2, -1)
                   AND start_minute < ?3 AND start_minute + duration_minutes > ?4)",
                params![date, id, start_minute + item.duration_minutes, start_minute],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| error.to_string())?
            != 0;
        if overlaps {
            return Err("这个时段与现有日程重叠".into());
        }
        let item_id = if let Some(id) = id {
            let changed = connection
                .execute(
                    "UPDATE scheduled_focus SET date=?1, source_name='手动安排', label=?2,
                     start_minute=?3, duration_minutes=?4, mode=?5,task_id=?7
                     WHERE id=?6 AND started_focus_id IS NULL",
                    params![
                        date,
                        item.label.trim(),
                        start_minute,
                        item.duration_minutes,
                        item.mode,
                        id,
                        item.task_id
                    ],
                )
                .map_err(|error| error.to_string())?;
            if changed == 0 {
                return Err("已开始或不存在的日程不能修改".into());
            }
            id
        } else {
            connection
                .execute(
                    "INSERT INTO scheduled_focus
                     (date, source_name, label, start_minute, duration_minutes, mode,task_id)
                     VALUES (?1, '手动安排', ?2, ?3, ?4, ?5,?6)",
                    params![
                        date,
                        item.label.trim(),
                        start_minute,
                        item.duration_minutes,
                        item.mode,
                        item.task_id
                    ],
                )
                .map_err(|error| error.to_string())?;
            connection.last_insert_rowid()
        };
        self.list_schedule(date)?
            .into_iter()
            .find(|entry| entry.id == item_id)
            .ok_or_else(|| "无法读取保存后的日程".into())
    }

    pub fn delete_schedule_item(&self, id: i64) -> Result<(), String> {
        let changed = self
            .connection()?
            .execute(
                "DELETE FROM scheduled_focus WHERE id=?1 AND started_focus_id IS NULL",
                params![id],
            )
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err("已开始或不存在的日程不能删除".into());
        }
        Ok(())
    }

    pub fn set_schedule_item_enabled(&self, id: i64, enabled: bool) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE scheduled_focus SET enabled = ?1 WHERE id = ?2 AND started_focus_id IS NULL",
                params![enabled as i64, id],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn set_schedule_day_enabled(&self, date: String, enabled: bool) -> Result<(), String> {
        NaiveDate::parse_from_str(&date, "%Y-%m-%d")
            .map_err(|_| "日期格式必须是 YYYY-MM-DD".to_string())?;
        self.connection()?
            .execute(
                "UPDATE scheduled_focus SET enabled = ?1
                 WHERE date = ?2 AND started_focus_id IS NULL",
                params![enabled as i64, date],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn upcoming_schedule_warnings(&self) -> Result<Vec<(i64, String, i64)>, String> {
        let now = Local::now();
        let tomorrow = now.date_naive() + chrono::Duration::days(1);
        let dates = [
            now.date_naive().format("%Y-%m-%d").to_string(),
            tomorrow.format("%Y-%m-%d").to_string(),
        ];
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, date, label, start_minute FROM scheduled_focus
                 WHERE enabled=1 AND started_focus_id IS NULL AND date IN (?1, ?2)",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![dates[0], dates[1]], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let mut warnings = Vec::new();
        for (id, date, label, minute) in rows {
            let date = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                .map_err(|_| "日程日期格式无效".to_string())?;
            let naive = date
                .and_hms_opt((minute / 60) as u32, (minute % 60) as u32, 0)
                .ok_or_else(|| "日程时间无效".to_string())?;
            let Some(start) = Local.from_local_datetime(&naive).earliest() else {
                continue;
            };
            let remaining = start.timestamp().saturating_sub(now.timestamp());
            if (1..=300).contains(&remaining) {
                warnings.push((id, label, remaining));
            }
        }
        Ok(warnings)
    }

    pub fn activate_due_schedule(&self) -> Result<Option<FocusState>, String> {
        self.activate_due_schedule_at(Utc::now().timestamp_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_storage(name: &str) -> Storage {
        let path = std::env::temp_dir().join(format!(
            "kedu-{name}-{}-{}.sqlite",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let storage = Storage::new(path).expect("temporary database");
        // Keep daily-usage fixtures stable when the suite happens to run just
        // after midnight: their synthetic two-minute segment must still fall
        // inside the configured day.
        let previous_hour = (Local::now().hour() + 23) % 24;
        storage
            .connection()
            .expect("temporary database connection")
            .execute(
                "UPDATE app_settings SET value=?1 WHERE key='day_start_hour'",
                params![previous_hour.to_string()],
            )
            .expect("set a stable test day boundary");
        storage
    }

    #[test]
    fn application_icon_path_prefers_an_exact_executable_match() {
        let storage = temporary_storage("icon-path");
        let connection = storage.connection().unwrap();
        connection
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle)
                 VALUES (1, 2, 'Visual Studio Code', 'C:\\Apps\\Code.exe', 'Editor', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle)
                 VALUES (3, 4, 'Codex', 'C:\\Apps\\Codex.exe', 'Agent', 0)",
                [],
            )
            .unwrap();

        assert_eq!(
            storage.latest_application_path("Code.exe").unwrap(),
            Some("C:\\Apps\\Code.exe".into())
        );
    }

    #[test]
    fn target_catalog_collects_recent_apps_sites_and_groups() {
        let storage = temporary_storage("target-catalog");
        let connection = storage.connection().unwrap();
        connection
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle)
                 VALUES (1000, 2000, 'Editor', 'C:\\Tools\\Editor.exe', 'File', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO web_activity_segments
                 (started_at, ended_at, domain, title, media_active, idle)
                 VALUES (1000, 2000, 'www.example.com', 'Example', 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO limit_rules
                 (name, target_kind, target_pattern, group_name, daily_limit_minutes)
                 VALUES ('Editor', 'application', 'Editor.exe', '工作;专注', 30)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO limit_rules
                 (name, target_kind, target_pattern, group_name, daily_limit_minutes)
                 VALUES ('工作组', 'group', '工作', '', 60)",
                [],
            )
            .unwrap();

        let catalog = storage.target_catalog().unwrap();
        assert_eq!(catalog.applications[0].display_name, "Editor");
        assert_eq!(catalog.applications[0].executable_name, "Editor.exe");
        assert_eq!(catalog.websites, vec!["example.com"]);
        assert_eq!(catalog.groups, vec!["专注", "工作"]);
    }

    #[test]
    fn focus_leave_is_cumulative_and_keeps_unused_balance() {
        let storage = temporary_storage("leave");
        let focus = storage
            .start_focus("学习".into(), 50, "normal".into())
            .unwrap();
        storage.start_focus_leave().unwrap();
        storage
            .connection()
            .unwrap()
            .execute(
                "UPDATE focus_sessions SET active_leave_started_at = ?1, phase_started_at = ?1 WHERE id = ?2",
                params![Utc::now().timestamp_millis() - 80_000, focus.id],
            )
            .unwrap();
        let resumed = storage.return_to_focus().unwrap();
        assert!((79..=81).contains(&resumed.leave_used_seconds));
        assert!((219..=221).contains(&resumed.leave_remaining_seconds));
        assert_eq!(resumed.status, "focusing");
    }

    #[test]
    fn strict_focus_rejects_ordinary_leave() {
        let storage = temporary_storage("strict");
        storage
            .start_focus("深度工作".into(), 25, "strict".into())
            .unwrap();
        assert!(storage
            .start_focus_leave()
            .unwrap_err()
            .contains("严格模式"));
    }

    #[test]
    fn relaxing_a_live_rule_waits_but_enabling_is_immediate() {
        let storage = temporary_storage("pending-rule");
        let rule = storage
            .create_rule(NewLimitRule {
                name: "游戏".into(),
                target_kind: "application".into(),
                target_pattern: "game.exe".into(),
                group_name: "游戏".into(),
                daily_limit_minutes: 60,
            })
            .unwrap();
        storage
            .start_focus("学习".into(), 25, "normal".into())
            .unwrap();
        let pending = storage.set_rule_enabled(rule.id, false).unwrap();
        assert!(pending.pending);
        assert!(
            storage
                .list_rules()
                .unwrap()
                .into_iter()
                .find(|item| item.id == rule.id)
                .unwrap()
                .enabled
        );
        storage
            .connection()
            .unwrap()
            .execute(
                "UPDATE pending_rule_changes SET apply_at=?1 WHERE rule_id=?2",
                params![Utc::now().timestamp_millis() - 1, rule.id],
            )
            .unwrap();
        assert_eq!(storage.apply_pending_rule_changes().unwrap(), 1);
        assert!(
            !storage
                .list_rules()
                .unwrap()
                .into_iter()
                .find(|item| item.id == rule.id)
                .unwrap()
                .enabled
        );
        assert!(!storage.set_rule_enabled(rule.id, true).unwrap().pending);
    }

    #[test]
    fn ending_focus_is_immediate_in_both_modes() {
        let storage = temporary_storage("pending-focus-end");
        for mode in ["normal", "strict"] {
            storage.start_focus("学习".into(), 25, mode.into()).unwrap();
            let ending = storage.end_focus().unwrap();
            assert!(ending.pending_end_at.is_none());
            assert_eq!(ending.status, "resting");
            assert!(storage.active_focus().unwrap().is_none());
            storage.skip_focus_break(ending.id).unwrap();
        }
    }

    #[test]
    fn settings_and_manual_schedule_items_round_trip() {
        let storage = temporary_storage("settings-schedule");
        let settings = storage
            .update_settings(AppSettings {
                idle_minutes: 8,
                count_media_while_idle: false,
                meeting_apps: "zoom;custom-meeting".into(),
                title_exclusions: "private.exe;bank.test".into(),
                day_start_hour: 4,
            })
            .unwrap();
        assert_eq!(settings.idle_minutes, 8);
        assert!(!settings.count_media_while_idle);
        assert_eq!(settings.day_start_hour, 4);
        let date = Local::now().format("%Y-%m-%d").to_string();
        let saved = storage
            .save_schedule_item(
                None,
                date.clone(),
                ScheduleImportItem {
                    task_id: None,
                    label: "写作".into(),
                    start: "08:30".into(),
                    duration_minutes: 45,
                    mode: "normal".into(),
                },
            )
            .unwrap();
        let updated = storage
            .save_schedule_item(
                Some(saved.id),
                date.clone(),
                ScheduleImportItem {
                    task_id: None,
                    label: "深度写作".into(),
                    start: "09:00".into(),
                    duration_minutes: 60,
                    mode: "strict".into(),
                },
            )
            .unwrap();
        assert_eq!(updated.label, "深度写作");
        assert_eq!(updated.mode, "strict");
        storage.delete_schedule_item(saved.id).unwrap();
        assert!(storage.list_schedule(date).unwrap().is_empty());
    }

    #[test]
    fn idle_transition_backfills_and_title_exclusions_apply() {
        let storage = temporary_storage("idle-backfill");
        storage
            .update_settings(AppSettings {
                idle_minutes: 5,
                count_media_while_idle: true,
                meeting_apps: String::new(),
                title_exclusions: "private.exe".into(),
                day_start_hour: 0,
            })
            .unwrap();
        let now = Utc::now().timestamp_millis();
        storage
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle)
                 VALUES (?1, ?2, 'private', 'C:\\private.exe', '秘密标题', 0)",
                params![now - 400_000, now],
            )
            .unwrap();
        storage
            .record_activity(&ActivityIdentity {
                app_name: "private".into(),
                process_path: "C:\\private.exe".into(),
                title: "不会保存".into(),
                idle: true,
                idle_seconds: 300,
                idle_exempt: false,
            })
            .unwrap();
        let connection = storage.connection().unwrap();
        let active_seconds = connection
            .query_row(
                "SELECT SUM(ended_at-started_at)/1000 FROM activity_segments WHERE idle=0",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert!((99..=101).contains(&active_seconds));
        let latest_title = connection
            .query_row(
                "SELECT title FROM activity_segments ORDER BY ended_at DESC, started_at DESC, id DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(latest_title.is_empty());
    }

    #[test]
    fn idle_web_activity_does_not_count_as_site_use() {
        let storage = temporary_storage("idle-web");
        let now = Utc::now().timestamp_millis();
        let start = now - 120_000;
        storage
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO activity_segments
             (started_at, ended_at, app_name, process_path, title, idle)
             VALUES (?1, ?2, 'Edge', 'C:\\msedge.exe', 'Idle', 0)",
                params![start, now],
            )
            .unwrap();
        storage
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO web_activity_segments
                 (started_at, ended_at, domain, title, media_active, idle)
                 VALUES (?1, ?2, 'idle.test', 'Idle', 0, 0)",
                params![start, now],
            )
            .unwrap();
        storage
            .record_web_activity(&WebActivity {
                domain: "idle.test".into(),
                title: "Idle".into(),
                media_active: false,
                idle: true,
                idle_seconds: 60,
                idle_exempt: false,
            })
            .unwrap();
        let stats = storage
            .statistics(start, Utc::now().timestamp_millis() + 1)
            .unwrap();
        assert!((59..=61).contains(&stats.site_usage[0].seconds));
        assert!(stats
            .timeline
            .iter()
            .all(|item| item.source_kind == "application"));
    }

    #[test]
    fn idle_backfill_preserves_media_exempt_time() {
        let storage = temporary_storage("media-idle-exempt");
        let now = Utc::now().timestamp_millis();
        let connection = storage.connection().unwrap();
        connection
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle)
                 VALUES (?1, ?2, 'msedge', 'C:\\msedge.exe', '课程', 0)",
                params![now - 600_000, now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO web_activity_segments
                 (started_at, ended_at, domain, title, media_active, idle)
                 VALUES (?1, ?2, 'course.test', '课程', 1, 0)",
                params![now - 600_000, now],
            )
            .unwrap();
        drop(connection);
        storage
            .record_activity(&ActivityIdentity {
                app_name: "msedge".into(),
                process_path: "C:\\msedge.exe".into(),
                title: "课程".into(),
                idle: false,
                idle_seconds: 600,
                idle_exempt: true,
            })
            .unwrap();
        storage
            .record_web_activity(&WebActivity {
                domain: "course.test".into(),
                title: "课程".into(),
                media_active: true,
                idle: false,
                idle_seconds: 600,
                idle_exempt: true,
            })
            .unwrap();
        storage
            .record_activity(&ActivityIdentity {
                app_name: "msedge".into(),
                process_path: "C:\\msedge.exe".into(),
                title: "课程".into(),
                idle: true,
                idle_seconds: 605,
                idle_exempt: false,
            })
            .unwrap();
        storage
            .record_web_activity(&WebActivity {
                domain: "course.test".into(),
                title: "课程".into(),
                media_active: false,
                idle: true,
                idle_seconds: 605,
                idle_exempt: false,
            })
            .unwrap();
        let stats = storage
            .statistics(now - 600_000, Utc::now().timestamp_millis() + 1)
            .unwrap();
        assert!((599..=601).contains(&stats.app_usage[0].seconds));
        assert!((599..=601).contains(&stats.site_usage[0].seconds));
    }

    #[test]
    fn emergency_history_reports_actual_target_use() {
        let storage = temporary_storage("exception-history");
        let rule = storage
            .create_rule(NewLimitRule {
                name: "聊天".into(),
                target_kind: "application".into(),
                target_pattern: "chat.exe".into(),
                group_name: String::new(),
                daily_limit_minutes: 10,
            })
            .unwrap();
        storage
            .record_exception(
                "daily".into(),
                rule.id.to_string(),
                String::new(),
                String::new(),
                5,
                "紧急使用".into(),
            )
            .unwrap();
        let now = Utc::now().timestamp_millis();
        let connection = storage.connection().unwrap();
        connection
            .execute(
                "UPDATE exceptions SET started_at=?1, ended_at=?2 WHERE target=?3",
                params![now - 30_000, now + 270_000, rule.id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle)
                 VALUES (?1, ?2, 'chat', 'C:\\chat.exe', '聊天', 0)",
                params![now - 30_000, now],
            )
            .unwrap();
        let history = storage.exception_history(10).unwrap();
        assert_eq!(history[0].rule_name, "聊天");
        assert!((29..=30).contains(&history[0].actual_seconds));
        assert_eq!(history[0].granted_seconds, 300);
    }

    #[test]
    fn active_exception_can_end_early() {
        let storage = temporary_storage("end-exception");
        let rule = storage
            .create_rule(NewLimitRule {
                name: "聊天".into(),
                target_kind: "application".into(),
                target_pattern: "chat.exe".into(),
                group_name: String::new(),
                daily_limit_minutes: 10,
            })
            .unwrap();
        storage
            .record_exception(
                "daily".into(),
                rule.id.to_string(),
                String::new(),
                String::new(),
                15,
                "临时处理".into(),
            )
            .unwrap();
        let before = storage.exception_history(1).unwrap().remove(0);
        assert!(before.ended_at > Utc::now().timestamp_millis());
        storage.end_exception(before.id).unwrap();
        let after = storage.exception_history(1).unwrap().remove(0);
        assert!(after.ended_at <= Utc::now().timestamp_millis());
        assert!(after.granted_seconds <= 1);
        assert!(storage.end_exception(after.id).is_err());
    }

    #[test]
    fn daily_and_focus_restrictions_require_separate_exceptions() {
        let storage = temporary_storage("layered-rules");
        let rule = storage
            .create_rule(NewLimitRule {
                name: "测试应用".into(),
                target_kind: "application".into(),
                target_pattern: "testapp.exe".into(),
                group_name: "测试组".into(),
                daily_limit_minutes: 1,
            })
            .unwrap();
        let now = Utc::now().timestamp_millis();
        storage
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle)
                 VALUES (?1, ?2, 'testapp', 'C:\\testapp.exe', 'test', 0)",
                params![now - 120_000, now],
            )
            .unwrap();
        storage
            .start_focus("学习".into(), 25, "normal".into())
            .unwrap();
        let activity = ActivityIdentity {
            app_name: "testapp".into(),
            process_path: "C:\\testapp.exe".into(),
            title: "test".into(),
            idle: false,
            idle_seconds: 0,
            idle_exempt: false,
        };
        let blocked = storage.restriction_for(&activity).unwrap().unwrap();
        assert_eq!(blocked.causes.len(), 2);
        storage
            .record_exception(
                "daily".into(),
                rule.id.to_string(),
                String::new(),
                String::new(),
                5,
                "测试".into(),
            )
            .unwrap();
        let still_blocked = storage.restriction_for(&activity).unwrap().unwrap();
        assert_eq!(still_blocked.causes.len(), 1);
        assert_eq!(still_blocked.causes[0].kind, "focus");
    }

    #[test]
    fn website_daily_quota_matches_subdomains_without_focus_restrictions() {
        let storage = temporary_storage("website-rules");
        let rule = storage
            .create_rule(NewLimitRule {
                name: "示例网站".into(),
                target_kind: "website".into(),
                target_pattern: "example.com".into(),
                group_name: "网站".into(),
                daily_limit_minutes: 1,
            })
            .unwrap();
        let now = Utc::now().timestamp_millis();
        storage
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO web_activity_segments
                 (started_at, ended_at, domain, title) VALUES (?1, ?2, 'video.example.com', '视频')",
                params![now - 120_000, now],
            )
            .unwrap();
        storage
            .start_focus("学习".into(), 25, "normal".into())
            .unwrap();
        let activity = WebActivity {
            domain: "www.video.example.com".into(),
            title: "视频".into(),
            media_active: false,
            idle: false,
            idle_seconds: 0,
            idle_exempt: false,
        };
        let blocked = storage.web_restriction_for(&activity).unwrap().unwrap();
        assert_eq!(blocked.causes.len(), 1);
        assert_eq!(blocked.causes[0].kind, "daily");
        storage
            .record_exception(
                "focus".into(),
                rule.id.to_string(),
                "website".into(),
                "video.example.com".into(),
                5,
                "测试".into(),
            )
            .unwrap();
        let still_blocked = storage.web_restriction_for(&activity).unwrap().unwrap();
        assert_eq!(still_blocked.causes.len(), 1);
        assert_eq!(still_blocked.causes[0].kind, "daily");
        assert!(storage
            .web_restriction_for(&WebActivity {
                domain: "unrelated.test".into(),
                title: String::new(),
                media_active: false,
                idle: false,
                idle_seconds: 0,
                idle_exempt: false,
            })
            .unwrap()
            .is_none());
        storage
            .record_exception(
                "daily".into(),
                rule.id.to_string(),
                "website".into(),
                "video.example.com".into(),
                5,
                "每日额度加时".into(),
            )
            .unwrap();
        assert!(storage.web_restriction_for(&activity).unwrap().is_none());
    }

    #[test]
    fn group_exception_only_releases_the_named_application() {
        let storage = temporary_storage("scoped-group-exception");
        for (name, pattern) in [("聊天一", "chat-one.exe"), ("聊天二", "chat-two.exe")] {
            storage
                .create_rule(NewLimitRule {
                    name: name.into(),
                    target_kind: "application".into(),
                    target_pattern: pattern.into(),
                    group_name: "聊天".into(),
                    daily_limit_minutes: 240,
                })
                .unwrap();
        }
        let group = storage
            .create_rule(NewLimitRule {
                name: "聊天组".into(),
                target_kind: "group".into(),
                target_pattern: "聊天".into(),
                group_name: String::new(),
                daily_limit_minutes: 1,
            })
            .unwrap();
        let now = Utc::now().timestamp_millis();
        storage
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle)
                 VALUES (?1, ?2, 'chat-one', 'C:\\chat-one.exe', '聊天', 0)",
                params![now - 120_000, now],
            )
            .unwrap();
        storage
            .record_exception(
                "daily".into(),
                group.id.to_string(),
                "application".into(),
                "chat-one".into(),
                15,
                "紧急使用".into(),
            )
            .unwrap();
        let activity = |name: &str| ActivityIdentity {
            app_name: name.into(),
            process_path: format!("C:\\{name}.exe"),
            title: "聊天".into(),
            idle: false,
            idle_seconds: 0,
            idle_exempt: false,
        };
        assert!(storage
            .restriction_for(&activity("chat-one"))
            .unwrap()
            .is_none());
        let second = storage
            .restriction_for(&activity("chat-two"))
            .unwrap()
            .unwrap();
        assert_eq!(second.causes.len(), 1);
        assert_eq!(second.causes[0].rule_id, group.id);
    }

    #[test]
    fn focus_exception_does_not_carry_into_the_next_focus() {
        let storage = temporary_storage("focus-exception-instance");
        let rule = storage
            .create_rule(NewLimitRule {
                name: "聊天".into(),
                target_kind: "application".into(),
                target_pattern: "chat.exe".into(),
                group_name: String::new(),
                daily_limit_minutes: 240,
            })
            .unwrap();
        let first = storage
            .start_focus("第一段".into(), 25, "normal".into())
            .unwrap();
        storage
            .record_exception(
                "focus".into(),
                rule.id.to_string(),
                String::new(),
                String::new(),
                15,
                "紧急使用".into(),
            )
            .unwrap();
        let activity = ActivityIdentity {
            app_name: "chat".into(),
            process_path: "C:\\chat.exe".into(),
            title: "聊天".into(),
            idle: false,
            idle_seconds: 0,
            idle_exempt: false,
        };
        assert!(storage.restriction_for(&activity).unwrap().is_none());
        storage.complete_focus(first.id).unwrap();
        storage
            .start_focus("第二段".into(), 25, "normal".into())
            .unwrap();
        let blocked = storage.restriction_for(&activity).unwrap().unwrap();
        assert_eq!(blocked.causes.len(), 1);
        assert_eq!(blocked.causes[0].kind, "focus");
    }

    #[test]
    fn overlapping_group_members_are_counted_once() {
        let storage = temporary_storage("overlapping-group");
        storage
            .create_rule(NewLimitRule {
                name: "Edge".into(),
                target_kind: "application".into(),
                target_pattern: "msedge.exe".into(),
                group_name: "娱乐;浏览".into(),
                daily_limit_minutes: 240,
            })
            .unwrap();
        storage
            .create_rule(NewLimitRule {
                name: "视频网站".into(),
                target_kind: "website".into(),
                target_pattern: "video.test".into(),
                group_name: "娱乐".into(),
                daily_limit_minutes: 240,
            })
            .unwrap();
        let group = storage
            .create_rule(NewLimitRule {
                name: "娱乐组".into(),
                target_kind: "group".into(),
                target_pattern: "娱乐".into(),
                group_name: String::new(),
                daily_limit_minutes: 1,
            })
            .unwrap();
        let now = Utc::now().timestamp_millis();
        let connection = storage.connection().unwrap();
        connection
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle)
                 VALUES (?1, ?2, 'msedge', 'C:\\msedge.exe', 'video', 0)",
                params![now - 120_000, now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO web_activity_segments
                 (started_at, ended_at, domain, title) VALUES (?1, ?2, 'video.test', 'video')",
                params![now - 120_000, now],
            )
            .unwrap();
        let refreshed = storage
            .list_rules()
            .unwrap()
            .into_iter()
            .find(|rule| rule.id == group.id)
            .unwrap();
        assert_eq!(refreshed.used_seconds, 120);
        let blocked = storage
            .restriction_for(&ActivityIdentity {
                app_name: "msedge".into(),
                process_path: "C:\\msedge.exe".into(),
                title: "video".into(),
                idle: false,
                idle_seconds: 0,
                idle_exempt: false,
            })
            .unwrap()
            .unwrap();
        assert_eq!(blocked.causes.len(), 1);
        assert_eq!(blocked.causes[0].rule_id, group.id);
    }

    #[test]
    fn imported_schedule_can_toggle_and_start_when_due() {
        let storage = temporary_storage("schedule");
        let local = Local::now();
        let date = local.format("%Y-%m-%d").to_string();
        let start = local.format("%H:%M").to_string();
        let items = storage
            .import_schedule(
                date.clone(),
                ScheduleImport {
                    name: "测试日程".into(),
                    items: vec![crate::domain::ScheduleImportItem {
                        task_id: None,
                        label: "到点专注".into(),
                        start,
                        duration_minutes: 10,
                        mode: "normal".into(),
                    }],
                },
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        storage
            .set_schedule_item_enabled(items[0].id, false)
            .unwrap();
        assert!(storage.activate_due_schedule().unwrap().is_none());
        storage
            .set_schedule_day_enabled(date.clone(), true)
            .unwrap();
        let focus = storage.activate_due_schedule().unwrap().unwrap();
        assert_eq!(focus.label, "到点专注");
        assert!(storage.list_schedule(date).unwrap()[0].started);
    }

    #[test]
    fn upcoming_schedule_warns_before_start_even_across_midnight() {
        let storage = temporary_storage("schedule-warning");
        let target = Local::now() + chrono::Duration::minutes(2);
        let item = storage
            .import_schedule(
                target.format("%Y-%m-%d").to_string(),
                ScheduleImport {
                    name: "提醒测试".into(),
                    items: vec![ScheduleImportItem {
                        task_id: None,
                        label: "准备学习".into(),
                        start: target.format("%H:%M").to_string(),
                        duration_minutes: 25,
                        mode: "normal".into(),
                    }],
                },
            )
            .unwrap()
            .remove(0);
        let warning = storage
            .upcoming_schedule_warnings()
            .unwrap()
            .into_iter()
            .find(|warning| warning.0 == item.id)
            .unwrap();
        assert_eq!(warning.1, "准备学习");
        assert!((30..=180).contains(&warning.2));
    }

    #[test]
    fn statistics_clamp_ranges_and_deduplicate_category_intervals() {
        let storage = temporary_storage("statistics");
        storage
            .create_classification_rule(NewClassificationRule {
                name: "浏览器".into(),
                target_kind: "application".into(),
                target_pattern: "browser.exe".into(),
                category: "学习".into(),
            })
            .unwrap();
        storage
            .create_classification_rule(NewClassificationRule {
                name: "标题纠正".into(),
                target_kind: "title".into(),
                target_pattern: "study".into(),
                category: "学习".into(),
            })
            .unwrap();
        storage
            .create_classification_rule(NewClassificationRule {
                name: "学习网站".into(),
                target_kind: "website".into(),
                target_pattern: "study.test".into(),
                category: "学习".into(),
            })
            .unwrap();
        let end = Utc::now().timestamp_millis();
        let start = end - 60_000;
        let connection = storage.connection().unwrap();
        connection
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle)
                 VALUES (?1, ?2, 'browser', 'C:\\browser.exe', 'study', 0)",
                params![end - 120_000, end],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO web_activity_segments
                 (started_at, ended_at, domain, title, browser_target) VALUES (?1, ?2, 'study.test', 'study', 'browser.exe')",
                params![end - 120_000, end],
            )
            .unwrap();
        let statistics = storage.statistics(start, end).unwrap();
        assert_eq!(statistics.effective_seconds, 60);
        assert_eq!(statistics.app_usage[0].seconds, 60);
        assert_eq!(statistics.site_usage[0].seconds, 60);
        assert_eq!(statistics.category_usage[0].seconds, 60);
        assert!(statistics
            .trend_series
            .iter()
            .any(|series| series.kind == "application" && series.name == "browser"));
        assert!(statistics
            .trend_series
            .iter()
            .any(|series| series.kind == "website" && series.name == "study.test"));
        assert!(statistics
            .trend_series
            .iter()
            .any(|series| series.kind == "category" && series.name == "学习"));
        assert_eq!(
            statistics
                .trend
                .iter()
                .map(|point| point.milliseconds)
                .sum::<i64>(),
            60_000
        );
    }

    #[test]
    fn backup_restore_and_csv_export_round_trip() {
        let storage = temporary_storage("backup");
        let now = Utc::now().timestamp_millis();
        storage
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO activity_segments
                 (started_at, ended_at, app_name, process_path, title, idle)
                 VALUES (?1, ?2, 'editor', 'C:\\editor.exe', '含逗号,和引号\"', 0)",
                params![now - 60_000, now],
            )
            .unwrap();
        let backup_path = std::env::temp_dir().join(format!(
            "kedu-test-backup-{}-{}.sqlite",
            std::process::id(),
            now
        ));
        let csv_path = std::env::temp_dir().join(format!(
            "kedu-test-export-{}-{}.csv",
            std::process::id(),
            now
        ));
        storage.backup_to(backup_path.clone()).unwrap();
        storage
            .connection()
            .unwrap()
            .execute("DELETE FROM activity_segments", [])
            .unwrap();
        storage.restore_from(backup_path.clone()).unwrap();
        let restored = storage
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM activity_segments", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(restored, 1);
        storage
            .export_csv(csv_path.clone(), now - 120_000, now + 1)
            .unwrap();
        let csv = fs::read_to_string(&csv_path).unwrap();
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("含逗号,和引号\"\""));
        let _ = fs::remove_file(backup_path);
        let _ = fs::remove_file(csv_path);
    }
}
