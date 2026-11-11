//! Focus is its own metric: completed learning intervals, never application activity.
//! Queries are read-only and use one SQLite snapshot for totals, slices and records.
use super::*;
use serde::{Deserialize, Serialize};

const COMPLETED: &str = "s.completed_at IS NOT NULL AND s.completion_reason IN ('manual','elapsed')
    AND s.phase IN ('resting','completed') AND s.completed_at>=s.started_at AND s.completed_at<=?3";
const MAX_TIMESTAMP: i64 = 253_402_300_799_999;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusStatisticsQuery {
    pub start: i64,
    pub end: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusRecordsQuery {
    pub start: i64,
    pub end: i64,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub task_key: Option<String>,
    pub search: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusTotals {
    pub completed_count: i64,
    pub milliseconds: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusLifetimeTotals {
    pub completed_count: i64,
    pub milliseconds: i64,
    pub average_milliseconds: i64,
    pub days_count: i64,
    pub first_started_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusTaskDistribution {
    pub key: String,
    pub task_id: Option<i64>,
    pub label: String,
    pub milliseconds: i64,
    pub completed_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusRecord {
    pub id: i64,
    pub task_key: String,
    pub task_id: Option<i64>,
    pub label: String,
    pub started_at: i64,
    pub completed_at: i64,
    pub milliseconds: i64,
    pub range_milliseconds: i64,
    pub pause_milliseconds: i64,
    pub timer_kind: String,
    pub mode: String,
    pub completion_reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusRecordsPage {
    pub items: Vec<FocusRecord>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusStatistics {
    pub start: i64,
    pub end: i64,
    pub generated_at: i64,
    pub total: FocusLifetimeTotals,
    pub range: FocusTotals,
    pub distribution: Vec<FocusTaskDistribution>,
    pub records: FocusRecordsPage,
}

struct CompletedSession {
    record: FocusRecord,
    distribution_label: String,
}

fn validate_range(start: i64, end: i64) -> Result<(), String> {
    if start < 0 || end <= start || end > MAX_TIMESTAMP {
        return Err("请选择有效的专注统计日期范围".into());
    }
    Ok(())
}

fn day_key(timestamp: i64, day_start_hour: u32) -> Result<NaiveDate, String> {
    let local = Local
        .timestamp_millis_opt(timestamp)
        .single()
        .ok_or("专注记录日期无效")?;
    let date = local.date_naive();
    Ok(if local.hour() < day_start_hour {
        date.pred_opt().ok_or("专注记录日期无效")?
    } else {
        date
    })
}

fn task_key(task_id: Option<i64>, label: &str) -> String {
    task_id.map_or_else(|| format!("label:{label}"), |id| format!("task:{id}"))
}

/// Preserve millisecond precision, merge duplicate/overlapping pauses, and never
/// let a display range invent more learning than the session actually recorded.
fn learning_in_range(
    started_at: i64,
    completed_at: i64,
    elapsed: i64,
    pauses: &[(i64, i64)],
    start: i64,
    end: i64,
) -> i64 {
    let mut cursor = started_at;
    let mut budget = elapsed.max(0);
    let mut total = 0i64;
    let mut take_interval = |from: i64, to: i64| {
        let duration = to.saturating_sub(from).max(0).min(budget);
        let counted_end = from.saturating_add(duration);
        total = total.saturating_add(counted_end.min(end).saturating_sub(from.max(start)).max(0));
        budget -= duration;
    };
    for &(pause_start, pause_end) in pauses {
        if pause_end <= cursor || pause_start >= completed_at || pause_end <= pause_start {
            continue;
        }
        let before_pause = pause_start.max(cursor).min(completed_at);
        take_interval(cursor, before_pause);
        cursor = cursor.max(pause_end.min(completed_at));
        if cursor >= completed_at {
            break;
        }
    }
    take_interval(cursor, completed_at.max(cursor));
    total
}

fn read_candidates(
    connection: &Connection,
    start: i64,
    end: i64,
    now: i64,
) -> Result<Vec<CompletedSession>, String> {
    let condition = format!("{COMPLETED} AND s.started_at<?2 AND s.completed_at>=?1");
    let mut pauses = BTreeMap::<i64, Vec<(i64, i64)>>::new();
    let mut statement = connection
        .prepare(&format!(
            "SELECT p.session_id,p.started_at,MIN(COALESCE(p.ended_at,s.completed_at),s.completed_at)
             FROM focus_pauses p JOIN focus_sessions s ON s.id=p.session_id
             WHERE {condition} ORDER BY p.session_id,p.started_at,p.id"
        ))
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![start, end, now], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (session, from, to) = row.map_err(|error| error.to_string())?;
        pauses.entry(session).or_default().push((from, to));
    }
    let mut statement = connection
        .prepare(&format!(
            "SELECT s.id,s.task_id,s.label,s.started_at,s.completed_at,MAX(0,s.elapsed_ms),
                    MAX(0,s.pause_used_ms),s.timer_kind,s.mode,s.completion_reason,
                    COALESCE(NULLIF(t.name,''),s.label)
             FROM focus_sessions s LEFT JOIN focus_tasks t ON t.id=s.task_id
             WHERE {condition} ORDER BY s.completed_at DESC,s.id DESC"
        ))
        .map_err(|error| error.to_string())?;
    let sessions = statement
        .query_map(params![start, end, now], |row| {
            let task_id: Option<i64> = row.get(1)?;
            let label: String = row.get(2)?;
            Ok(CompletedSession {
                record: FocusRecord {
                    id: row.get(0)?,
                    task_key: task_key(task_id, &label),
                    task_id,
                    label,
                    started_at: row.get(3)?,
                    completed_at: row.get(4)?,
                    milliseconds: row.get(5)?,
                    range_milliseconds: 0,
                    pause_milliseconds: row.get(6)?,
                    timer_kind: row.get(7)?,
                    mode: row.get(8)?,
                    completion_reason: row.get(9)?,
                },
                distribution_label: row.get(10)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(sessions
        .into_iter()
        .filter_map(|mut session| {
            let record = &mut session.record;
            record.range_milliseconds = learning_in_range(
                record.started_at,
                record.completed_at,
                record.milliseconds,
                pauses
                    .get(&record.id)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                start,
                end,
            );
            // Include a zero-duration completion, but omit a session whose only
            // overlap with this range was a pause (and completed on another day).
            (record.range_milliseconds > 0 || (start..end).contains(&record.completed_at))
                .then_some(session)
        })
        .collect())
}

fn page_records(
    sessions: Vec<CompletedSession>,
    offset: usize,
    limit: usize,
    task: Option<&str>,
    search: Option<&str>,
) -> FocusRecordsPage {
    let search = search.unwrap_or_default().trim().to_lowercase();
    let records = sessions
        .into_iter()
        .filter(|session| {
            task.is_none_or(|task| task == session.record.task_key)
                && (search.is_empty()
                    || session.record.label.to_lowercase().contains(&search)
                    || session.distribution_label.to_lowercase().contains(&search))
        })
        .map(|session| session.record)
        .collect::<Vec<_>>();
    let total = records.len();
    FocusRecordsPage {
        items: records.into_iter().skip(offset).take(limit).collect(),
        total,
        offset,
        limit,
    }
}

impl Storage {
    pub fn query_focus_statistics(
        &self,
        query: FocusStatisticsQuery,
    ) -> Result<FocusStatistics, String> {
        self.query_focus_statistics_at(query, Utc::now().timestamp_millis())
    }

    fn query_focus_statistics_at(
        &self,
        query: FocusStatisticsQuery,
        now: i64,
    ) -> Result<FocusStatistics, String> {
        validate_range(query.start, query.end)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        // Aggregate lifetime data in SQL; detailed pauses are only loaded for
        // sessions touching the selected range, not the entire activity history.
        let (completed_count, milliseconds, first_started_at) = transaction
            .query_row(
                &format!(
                    "SELECT COUNT(*),COALESCE(SUM(MAX(0,s.elapsed_ms)),0),
                         MIN(CASE WHEN s.elapsed_ms>0 THEN s.started_at ELSE s.completed_at END)
                     FROM focus_sessions s WHERE {COMPLETED}"
                ),
                params![query.start, query.end, now],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .map_err(|error| error.to_string())?;
        let day_start = transaction
            .query_row(
                "SELECT value FROM app_settings WHERE key='day_start_hour'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
            .min(23);
        let days_count = if let Some(first) = first_started_at {
            (day_key(now, day_start)? - day_key(first, day_start)?)
                .num_days()
                .saturating_add(1)
                .max(1)
        } else {
            0
        };
        let sessions = read_candidates(&transaction, query.start, query.end, now)?;
        let mut range = FocusTotals::default();
        let mut distribution = BTreeMap::<String, FocusTaskDistribution>::new();
        for session in &sessions {
            let record = &session.record;
            let count = i64::from((query.start..query.end).contains(&record.completed_at));
            range.completed_count += count;
            range.milliseconds = range.milliseconds.saturating_add(record.range_milliseconds);
            let entry = distribution
                .entry(record.task_key.clone())
                .or_insert_with(|| FocusTaskDistribution {
                    key: record.task_key.clone(),
                    task_id: record.task_id,
                    label: session.distribution_label.clone(),
                    milliseconds: 0,
                    completed_count: 0,
                });
            entry.milliseconds = entry.milliseconds.saturating_add(record.range_milliseconds);
            entry.completed_count += count;
        }
        let mut distribution = distribution.into_values().collect::<Vec<_>>();
        distribution.sort_by(|left, right| {
            right
                .milliseconds
                .cmp(&left.milliseconds)
                .then_with(|| left.key.cmp(&right.key))
        });
        let records = page_records(sessions, 0, 20, None, None);
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(FocusStatistics {
            start: query.start,
            end: query.end,
            generated_at: now,
            total: FocusLifetimeTotals {
                completed_count,
                milliseconds,
                average_milliseconds: if days_count > 0 {
                    milliseconds / days_count
                } else {
                    0
                },
                days_count,
                first_started_at,
            },
            range,
            distribution,
            records,
        })
    }

    pub fn list_focus_records(&self, query: FocusRecordsQuery) -> Result<FocusRecordsPage, String> {
        validate_range(query.start, query.end)?;
        let limit = query.limit.unwrap_or(20);
        if !(1..=100).contains(&limit) {
            return Err("每页专注记录数量需要在 1 到 100 之间".into());
        }
        if query
            .search
            .as_ref()
            .is_some_and(|value| value.chars().count() > 200)
        {
            return Err("搜索内容不能超过 200 个字".into());
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let sessions = read_candidates(
            &transaction,
            query.start,
            query.end,
            Utc::now().timestamp_millis(),
        )?;
        let result = page_records(
            sessions,
            query.offset.unwrap_or(0),
            limit,
            query.task_key.as_deref().filter(|key| !key.is_empty()),
            query.search.as_deref(),
        );
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> Storage {
        Storage::new(std::env::temp_dir().join(format!(
            "kedu-focus-statistics-{name}-{}-{}.sqlite",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        )))
        .unwrap()
    }

    fn at(day: u32, hour: u32, minute: u32) -> i64 {
        Local
            .with_ymd_and_hms(2026, 1, day, hour, minute, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    }

    fn completed(
        storage: &Storage,
        label: &str,
        task_id: Option<i64>,
        start: i64,
        end: i64,
        pauses: &[(i64, i64)],
    ) -> i64 {
        let connection = storage.connection().unwrap();
        let paused = pauses.iter().map(|(from, to)| to - from).sum::<i64>();
        let elapsed = end - start - paused;
        connection.execute(
            "INSERT INTO focus_sessions(label,mode,started_at,planned_end,ended_at,task_id,
                 timer_kind,duration_ms,elapsed_ms,phase,phase_started_at,pause_used_ms,
                 leave_used_seconds,completed_at,completion_reason,break_ends_at)
             VALUES(?1,'normal',?2,?3,?3,?4,'countdown',?5,?5,'completed',?3,?6,?7,?3,'manual',?3+300000)",
            params![label,start,end,task_id,elapsed,paused,paused/1000]
        ).unwrap();
        let id = connection.last_insert_rowid();
        for (from, to) in pauses {
            connection
                .execute(
                    "INSERT INTO focus_pauses(session_id,started_at,ended_at) VALUES(?1,?2,?3)",
                    params![id, from, to],
                )
                .unwrap();
        }
        id
    }

    fn query(storage: &Storage, start: i64, end: i64, now: i64) -> FocusStatistics {
        storage
            .query_focus_statistics_at(FocusStatisticsQuery { start, end }, now)
            .unwrap()
    }

    #[test]
    fn empty_history_has_no_invented_daily_average() {
        let storage = store("empty");
        let result = query(&storage, at(1, 0, 0), at(2, 0, 0), at(2, 12, 0));
        assert_eq!(result.total.completed_count, 0);
        assert_eq!(result.total.days_count, 0);
        assert_eq!(result.total.average_milliseconds, 0);
        assert!(result.total.first_started_at.is_none());
        assert!(result.distribution.is_empty());
        assert!(result.records.items.is_empty());
    }

    #[test]
    fn half_open_ranges_count_completion_once_and_keep_fractional_time() {
        let storage = store("boundaries");
        let start = at(1, 10, 0);
        let end = start + 40_321;
        completed(&storage, "短时学习", None, start, end, &[]);
        let before = query(&storage, start - 1000, start, end + 1000);
        assert_eq!(before.range.milliseconds, 0);
        let first = query(&storage, start, end, end + 1000);
        assert_eq!(first.range.milliseconds, 40_321);
        assert_eq!(first.range.completed_count, 0);
        let after = query(&storage, end, end + 1000, end + 1000);
        assert_eq!(after.range.milliseconds, 0);
        assert_eq!(after.range.completed_count, 1);
        assert_eq!(after.records.total, 1);
        let all = query(&storage, start, end + 1, end + 1000);
        assert_eq!(all.total.milliseconds, 40_321);
        assert_eq!(all.range.milliseconds, all.total.milliseconds);
        assert_eq!(all.distribution[0].milliseconds, 40_321);
        assert_eq!(all.records.items[0].milliseconds, 40_321);
    }

    #[test]
    fn cross_day_learning_excludes_pauses_and_rest_with_calendar_average() {
        let storage = store("cross-day");
        let start = at(1, 23, 58);
        let midnight = at(2, 0, 0);
        let end = at(2, 0, 3);
        completed(
            &storage,
            "数学",
            None,
            start,
            end,
            &[(midnight, midnight + 60_000)],
        );
        let now = at(3, 10, 0);
        let day_one = query(&storage, at(1, 0, 0), midnight, now);
        let day_two = query(&storage, midnight, at(3, 0, 0), now);
        assert_eq!(day_one.range.completed_count, 0);
        assert_eq!(day_two.range.completed_count, 1);
        assert_eq!(day_one.range.milliseconds, 120_000);
        assert_eq!(day_two.range.milliseconds, 120_000);
        assert_eq!(day_two.total.milliseconds, 240_000);
        assert_eq!(day_two.total.days_count, 3);
        assert_eq!(day_two.total.average_milliseconds, 80_000);
        assert_eq!(day_two.records.items[0].pause_milliseconds, 60_000);
        assert_eq!(day_two.records.items[0].range_milliseconds, 120_000);
        let pause_only = query(&storage, midnight, midnight + 60_000, now);
        assert_eq!(pause_only.range.milliseconds, 0);
        assert_eq!(pause_only.records.total, 0);
        let rest = query(&storage, end + 1, end + 300_000, now);
        assert_eq!(rest.range.milliseconds, 0);
        assert_eq!(rest.range.completed_count, 0);
    }

    #[test]
    fn day_start_hour_changes_average_day_count_without_changing_durations() {
        let storage = store("day-start");
        storage
            .connection()
            .unwrap()
            .execute(
                "UPDATE app_settings SET value='4' WHERE key='day_start_hour'",
                [],
            )
            .unwrap();
        completed(&storage, "早晨学习", None, at(2, 3, 58), at(2, 4, 2), &[]);
        let result = query(&storage, at(1, 4, 0), at(2, 4, 0), at(3, 3, 0));
        assert_eq!(result.range.milliseconds, 120_000);
        assert_eq!(result.range.completed_count, 0);
        assert_eq!(result.total.days_count, 2);
        assert_eq!(result.total.average_milliseconds, 120_000);
    }

    #[test]
    fn task_rename_and_deletion_preserve_identity_and_original_labels() {
        let storage = store("task-history");
        let task = storage
            .save_focus_task(crate::domain::FocusTaskInput {
                id: None,
                name: "旧名".into(),
                timer_kind: "countdown".into(),
                duration_minutes: 25,
                mode: "normal".into(),
            })
            .unwrap();
        completed(
            &storage,
            "旧名",
            Some(task.id),
            at(1, 9, 0),
            at(1, 9, 1),
            &[],
        );
        storage
            .save_focus_task(crate::domain::FocusTaskInput {
                id: Some(task.id),
                name: "新名".into(),
                timer_kind: "countdown".into(),
                duration_minutes: 35,
                mode: "normal".into(),
            })
            .unwrap();
        completed(
            &storage,
            "新名",
            Some(task.id),
            at(1, 10, 0),
            at(1, 10, 1),
            &[],
        );
        storage.delete_focus_task(task.id).unwrap();
        let result = query(&storage, at(1, 0, 0), at(2, 0, 0), at(2, 10, 0));
        assert_eq!(result.distribution.len(), 1);
        assert_eq!(result.distribution[0].key, format!("task:{}", task.id));
        assert_eq!(result.distribution[0].label, "新名");
        assert_eq!(result.distribution[0].completed_count, 2);
        assert_eq!(result.records.items[0].label, "新名");
        assert_eq!(result.records.items[1].label, "旧名");
        let filtered = storage
            .list_focus_records(FocusRecordsQuery {
                start: at(1, 0, 0),
                end: at(2, 0, 0),
                offset: None,
                limit: None,
                task_key: Some(format!("task:{}", task.id)),
                search: Some("旧名".into()),
            })
            .unwrap();
        assert_eq!(filtered.total, 1);
        assert_eq!(filtered.items[0].label, "旧名");
    }

    #[test]
    fn unlinked_tasks_and_zero_duration_completion_are_not_lost() {
        let storage = store("unlinked");
        completed(&storage, "英语", None, at(1, 9, 0), at(1, 9, 1), &[]);
        completed(&storage, "数学", None, at(1, 10, 0), at(1, 10, 0), &[]);
        let result = query(&storage, at(1, 0, 0), at(2, 0, 0), at(2, 12, 0));
        assert_eq!(result.total.completed_count, 2);
        assert_eq!(result.total.milliseconds, 60_000);
        assert_eq!(result.distribution.len(), 2);
        assert_eq!(result.distribution[1].key, "label:数学");
        assert_eq!(result.distribution[1].milliseconds, 0);
        assert_eq!(result.distribution[1].completed_count, 1);
    }

    #[test]
    fn running_legacy_and_future_sessions_do_not_fabricate_completions() {
        let storage = store("unreliable");
        let connection = storage.connection().unwrap();
        connection.execute("INSERT INTO focus_sessions(label,mode,started_at,planned_end,ended_at,phase,elapsed_ms) VALUES('旧数据','normal',?1,?1+60000,?1+60000,'completed',60000)",params![at(1,9,0)]).unwrap();
        connection.execute("INSERT INTO focus_sessions(label,mode,started_at,planned_end,phase,phase_started_at,elapsed_ms) VALUES('进行中','normal',?1,?1+60000,'focusing',?1,40000)",params![at(1,10,0)]).unwrap();
        completed(&storage, "未来记录", None, at(4, 9, 0), at(4, 10, 0), &[]);
        let result = query(&storage, at(1, 0, 0), at(5, 0, 0), at(2, 12, 0));
        assert_eq!(result.total.completed_count, 0);
        assert_eq!(result.range.milliseconds, 0);
        assert_eq!(result.records.total, 0);
        let phase: String = connection
            .query_row(
                "SELECT phase FROM focus_sessions WHERE label='进行中'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            phase, "focusing",
            "Reading statistics must never advance a session"
        );
    }

    #[test]
    fn record_pagination_is_complete_and_does_not_change_summary() {
        let storage = store("pagination");
        let start = at(1, 9, 0);
        for index in 0..53 {
            completed(
                &storage,
                "重复学习",
                None,
                start + index * 120_000,
                start + index * 120_000 + 1_001,
                &[],
            );
        }
        let summary = query(&storage, at(1, 0, 0), at(2, 0, 0), at(2, 12, 0));
        assert_eq!(summary.range.completed_count, 53);
        assert_eq!(summary.range.milliseconds, 53 * 1_001);
        assert_eq!(summary.records.items.len(), 20);
        assert_eq!(summary.records.total, 53);
        let mut ids = BTreeSet::new();
        for offset in [0, 20, 40] {
            let page = storage
                .list_focus_records(FocusRecordsQuery {
                    start: at(1, 0, 0),
                    end: at(2, 0, 0),
                    offset: Some(offset),
                    limit: Some(20),
                    task_key: None,
                    search: None,
                })
                .unwrap();
            assert_eq!(page.total, 53);
            for row in page.items {
                assert!(ids.insert(row.id));
            }
        }
        assert_eq!(ids.len(), 53);
        let beyond = storage
            .list_focus_records(FocusRecordsQuery {
                start: at(1, 0, 0),
                end: at(2, 0, 0),
                offset: Some(999),
                limit: Some(20),
                task_key: None,
                search: None,
            })
            .unwrap();
        assert_eq!(beyond.total, 53);
        assert!(beyond.items.is_empty());
    }

    #[test]
    fn invalid_queries_are_rejected_and_pauses_do_not_double_subtract() {
        let storage = store("invalid");
        assert!(storage
            .query_focus_statistics(FocusStatisticsQuery { start: 2, end: 1 })
            .is_err());
        assert!(storage
            .query_focus_statistics(FocusStatisticsQuery { start: -1, end: 1 })
            .is_err());
        assert!(storage
            .list_focus_records(FocusRecordsQuery {
                start: 0,
                end: 100,
                offset: None,
                limit: Some(0),
                task_key: None,
                search: None
            })
            .is_err());
        assert_eq!(
            learning_in_range(0, 100, 50, &[(10, 30), (20, 40), (50, 70)], 0, 100),
            50
        );
        assert_eq!(
            learning_in_range(0, 100, 50, &[(10, 30), (20, 40), (50, 70)], 10, 70),
            10
        );
        assert_eq!(
            learning_in_range(0, 100, 50, &[(10, 30), (20, 40), (50, 70)], 100, 200),
            0
        );
        assert_eq!(learning_in_range(0, 100, -20, &[], 0, 100), 0);
        assert_eq!(learning_in_range(0, 100, 1000, &[], 0, 100), 100);
    }
}
