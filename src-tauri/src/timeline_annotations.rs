//! Interval corrections overlay foreground history; raw activity is never rewritten.
use super::*;
use crate::{domain::StatisticsQuery, timeline_models::*};

#[derive(Clone)]
pub(super) struct Annotation {
    pub id: i64,
    pub start: i64,
    pub end: i64,
    pub category: Option<String>,
    pub revision: i64,
}

pub(super) fn migrate(conn: &Connection, version: i64) -> Result<(), String> {
    if version >= 8 {
        validate(conn)?;
    }
    conn.execute_batch("CREATE TABLE IF NOT EXISTS timeline_annotation_batches(id INTEGER PRIMARY KEY AUTOINCREMENT,created_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS timeline_annotations(id INTEGER PRIMARY KEY AUTOINCREMENT,batch_id INTEGER NOT NULL REFERENCES timeline_annotation_batches(id) ON DELETE CASCADE,started_at INTEGER NOT NULL,ended_at INTEGER NOT NULL,category TEXT,updated_at INTEGER NOT NULL,CHECK(started_at>=0 AND ended_at>started_at));
        CREATE INDEX IF NOT EXISTS timeline_annotation_time ON timeline_annotations(ended_at,started_at);
        CREATE TABLE IF NOT EXISTS timeline_annotation_undo(id INTEGER PRIMARY KEY CHECK(id=1),batch_id INTEGER NOT NULL REFERENCES timeline_annotation_batches(id) ON DELETE CASCADE);").map_err(|e|e.to_string())?;
    Ok(())
}
pub(super) fn validate(conn: &Connection) -> Result<(), String> {
    conn.prepare("SELECT id,created_at FROM timeline_annotation_batches")
        .map_err(|e| format!("备份缺少时间轴标注：{e}"))?;
    conn.prepare(
        "SELECT id,batch_id,started_at,ended_at,category,updated_at FROM timeline_annotations",
    )
    .map_err(|e| e.to_string())?;
    conn.prepare("SELECT id,batch_id FROM timeline_annotation_undo")
        .map_err(|e| e.to_string())?;
    let invalid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM timeline_annotations a LEFT JOIN timeline_annotation_batches b ON a.batch_id=b.id WHERE b.id IS NULL OR a.started_at<0 OR a.ended_at<=a.started_at OR a.ended_at>253402300799999 OR a.updated_at<0 OR (a.category IS NOT NULL AND a.category<>'未分类' AND NOT EXISTS(SELECT 1 FROM stat_categories c WHERE c.name=a.category))) OR EXISTS(SELECT 1 FROM timeline_annotation_undo u LEFT JOIN timeline_annotation_batches b ON u.batch_id=b.id WHERE u.id<>1 OR b.id IS NULL)",[],|r|r.get(0)).map_err(|e|e.to_string())?;
    if invalid {
        return Err("备份中的时间轴标注无效".into());
    }
    Ok(())
}
pub(super) fn load(conn: &Connection, start: i64, end: i64) -> Result<Vec<Annotation>, String> {
    let mut q=conn.prepare("SELECT id,started_at,ended_at,category,updated_at FROM timeline_annotations WHERE ended_at>?1 AND started_at<?2 ORDER BY started_at,id").map_err(|e|e.to_string())?;
    let rows = q
        .query_map(params![start, end], |r| {
            Ok(Annotation {
                id: r.get(0)?,
                start: r.get(1)?,
                end: r.get(2)?,
                category: r.get(3)?,
                revision: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string());
    rows
}
fn available(conn: &Connection) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM timeline_annotation_undo)",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}
fn query(start: i64, end: i64) -> StatisticsQuery {
    StatisticsQuery {
        start,
        end,
        bucket: "hour".into(),
        target_kind: None,
        target: None,
        offset: None,
        limit: None,
        search: None,
        include_excluded: None,
        page_key: None,
    }
}
fn checked_range(start: i64, end: i64) -> Result<(), String> {
    if start < 0 || end <= start || end > 253402300799999 || end - start > 32 * 86_400_000 {
        Err("请选择有效的时间范围（最多 32 天）".into())
    } else {
        Ok(())
    }
}
fn normalized(mut ranges: Vec<TimeRange>) -> Result<Vec<TimeRange>, String> {
    if ranges.is_empty() || ranges.len() > 10000 {
        return Err("请选择 1–10000 个有效时间段".into());
    }
    for r in &ranges {
        checked_range(r.start, r.end)?;
    }
    ranges.sort_by_key(|r| r.start);
    checked_range(ranges[0].start, ranges.iter().map(|r| r.end).max().unwrap())?;
    let mut merged: Vec<TimeRange> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut() {
            if range.start <= last.end {
                last.end = last.end.max(range.end);
                continue;
            }
        }
        merged.push(range);
    }
    Ok(merged)
}
impl Storage {
    pub fn timeline_workspace(&self, start: i64, end: i64) -> Result<TimelineWorkspace, String> {
        checked_range(start, end)?;
        let c = self.connection()?;
        c.execute_batch("BEGIN").map_err(|e| e.to_string())?;
        Ok(TimelineWorkspace {
            items: Self::timeline_items_on(&c, &query(start, end))?,
            categories: category_management::list_categories(&c)?,
            undo_available: available(&c)?,
        })
    }
    pub fn annotate_timeline(
        &self,
        ranges: Vec<TimeRange>,
        category_id: Option<i64>,
        only_unclassified: bool,
    ) -> Result<AnnotationResult, String> {
        let ranges = normalized(ranges)?;
        let mut c = self.connection()?;
        let tx = c
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let category = match category_id {
            None => None,
            Some(0) => Some("未分类".to_string()),
            Some(id) => Some(
                category_management::list_categories(&tx)?
                    .into_iter()
                    .find(|c| c.id == id)
                    .ok_or("分类已不存在，请刷新")?
                    .name,
            ),
        };
        let slices =
            Self::timeline_candidates_on(&tx, &query(ranges[0].start, ranges.last().unwrap().end))?;
        let mut clipped: Vec<TimeRange> = Vec::new();
        let mut cursor = 0;
        for slice in slices {
            if only_unclassified && slice.category != "未分类" {
                continue;
            }
            // An explicit label pins history even when today's automatic rule
            // already produces that label. A later default change must not
            // silently reclassify the portion the user deliberately selected.
            if (slice.manual
                && category
                    .as_ref()
                    .is_some_and(|name| name == &slice.category))
                || (category.is_none() && !slice.manual)
            {
                continue;
            }
            while cursor < ranges.len() && ranges[cursor].end <= slice.start {
                cursor += 1;
            }
            for range in ranges[cursor..].iter().take_while(|r| r.start < slice.end) {
                let part = TimeRange {
                    start: slice.start.max(range.start),
                    end: slice.end.min(range.end),
                };
                if part.end > part.start {
                    if let Some(last) = clipped.last_mut() {
                        if last.end == part.start {
                            last.end = part.end;
                            continue;
                        }
                    }
                    clipped.push(part);
                }
            }
        }
        let changed_milliseconds = clipped.iter().map(|r| r.end - r.start).sum();
        if !clipped.is_empty() {
            let revision = category_management::next_revision(&tx)?;
            tx.execute(
                "INSERT INTO timeline_annotation_batches(created_at) VALUES(?1)",
                [revision],
            )
            .map_err(|e| e.to_string())?;
            let batch = tx.last_insert_rowid();
            for r in clipped {
                tx.execute("INSERT INTO timeline_annotations(batch_id,started_at,ended_at,category,updated_at) VALUES(?1,?2,?3,?4,?5)",params![batch,r.start,r.end,category,revision]).map_err(|e|e.to_string())?;
            }
            tx.execute("INSERT INTO timeline_annotation_undo(id,batch_id) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET batch_id=excluded.batch_id",[batch]).map_err(|e|e.to_string())?;
        }
        let undo_available = available(&tx)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(AnnotationResult {
            changed_milliseconds,
            undo_available,
        })
    }
    pub fn undo_timeline_annotation(&self) -> Result<TimelineUndoResult, String> {
        let mut c = self.connection()?;
        let tx = c
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let batch: Option<i64> = tx
            .query_row(
                "SELECT batch_id FROM timeline_annotation_undo WHERE id=1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(batch) = batch else {
            return Ok(TimelineUndoResult {
                restored_milliseconds: 0,
            });
        };
        let (start, end): (i64, i64) = tx
            .query_row(
                "SELECT MIN(started_at),MAX(ended_at) FROM timeline_annotations WHERE batch_id=?1",
                [batch],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
        let mut q = tx
            .prepare("SELECT id FROM timeline_annotations WHERE batch_id=?1")
            .map_err(|e| e.to_string())?;
        let ids = q
            .query_map([batch], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(|e| e.to_string())?;
        drop(q);
        let restored_milliseconds = Self::timeline_candidates_on(&tx, &query(start, end))?
            .into_iter()
            .filter(|s| s.annotation_id.is_some_and(|id| ids.contains(&id)))
            .map(|s| s.end - s.start)
            .sum();
        tx.execute(
            "DELETE FROM timeline_annotation_batches WHERE id=?1",
            [batch],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(TimelineUndoResult {
            restored_milliseconds,
        })
    }
}

#[cfg(test)]
#[path = "timeline_tests.rs"]
mod tests;
