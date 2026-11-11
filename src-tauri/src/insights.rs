//! Habit insights derived from the same foreground partition as every statistic.
//! Nothing here writes; excluded targets and idle time follow the regular rules.
use super::*;
use serde::Serialize;

/// Contiguous foreground slices separated by at most this gap form one stretch.
const STRETCH_GAP_MILLISECONDS: i64 = 1_000;
/// A statistical day needs at least a minute of use to count as active.
const ACTIVE_DAY_MILLISECONDS: i64 = 60_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InsightDay {
    pub started_at: i64,
    pub ended_at: i64,
    pub label: String,
    pub milliseconds: i64,
    /// First and last non-idle moment of the day, `None` without activity.
    pub first_activity_at: Option<i64>,
    pub last_activity_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InsightStretch {
    pub started_at: i64,
    pub ended_at: i64,
    pub milliseconds: i64,
    /// Present for single-application sessions.
    pub name: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageInsights {
    pub start: i64,
    pub end: i64,
    pub effective_milliseconds: i64,
    pub idle_milliseconds: i64,
    /// The previous period covers the same elapsed span, so "today so far" is
    /// compared with the same hours of yesterday rather than a whole day.
    pub previous_start: i64,
    pub previous_end: i64,
    pub previous_effective_milliseconds: i64,
    pub day_count: usize,
    pub active_days: usize,
    pub days: Vec<InsightDay>,
    /// Monday-first weekday rows, 24 local clock hours each.
    pub heatmap: Vec<Vec<i64>>,
    pub hourly: Vec<i64>,
    pub peak_hour: Option<u32>,
    pub longest_stretch: Option<InsightStretch>,
    pub longest_session: Option<InsightStretch>,
    pub switches: i64,
    pub switches_per_hour: f64,
    /// Average minutes after local midnight; can exceed 1440 with a late day start.
    pub average_first_minute: Option<i64>,
    pub average_last_minute: Option<i64>,
}

fn add_by_local_hour(heatmap: &mut [Vec<i64>], hourly: &mut [i64], start: i64, end: i64) {
    let mut cursor = start;
    while cursor < end {
        let Some(moment) = Local.timestamp_millis_opt(cursor).single() else {
            return;
        };
        let into_hour = moment.minute() as i64 * 60_000
            + moment.second() as i64 * 1000
            + moment.timestamp_subsec_millis() as i64;
        let next = (cursor + 3_600_000 - into_hour).min(end).max(cursor + 1);
        let hour = moment.hour() as usize;
        let weekday = moment.weekday().num_days_from_monday() as usize;
        heatmap[weekday][hour] += next - cursor;
        hourly[hour] += next - cursor;
        cursor = next;
    }
}

fn minute_of_day(time: i64, day_start: i64, day_start_hour: u32) -> i64 {
    (time - day_start) / 60_000 + day_start_hour as i64 * 60
}

fn average(values: &[i64]) -> Option<i64> {
    (!values.is_empty()).then(|| values.iter().sum::<i64>() / values.len() as i64)
}

impl Storage {
    pub fn query_insights(&self, start: i64, end: i64, now: i64) -> Result<UsageInsights, String> {
        if end <= start {
            return Err("无效的统计时间范围".into());
        }
        if end - start > 400 * 86_400_000 {
            return Err("洞察范围不能超过 400 天".into());
        }
        let day_start_hour = self.settings()?.day_start_hour as u32;
        let query = |start: i64, end: i64| StatisticsQuery {
            start,
            end,
            bucket: "day".into(),
            target_kind: None,
            target: None,
            offset: None,
            limit: None,
            search: None,
            include_excluded: None,
            page_key: None,
        };
        let current = query(start, end);
        let mut slices = self
            .statistic_slices(&current)?
            .into_iter()
            .filter(|slice| !slice.excluded)
            .collect::<Vec<_>>();
        slices.sort_by_key(|slice| (slice.start, slice.end));

        let mut day_points = buckets(&current, day_start_hour)?;
        let mut firsts = vec![None::<i64>; day_points.len()];
        let mut lasts = vec![None::<i64>; day_points.len()];
        let mut heatmap = vec![vec![0_i64; 24]; 7];
        let mut hourly = vec![0_i64; 24];
        let mut effective = 0;
        let mut idle = 0;
        let mut switches = 0;
        let mut previous_target: Option<&str> = None;
        let mut stretch: Option<InsightStretch> = None;
        let mut session: Option<InsightStretch> = None;
        let mut longest_stretch: Option<InsightStretch> = None;
        let mut longest_session: Option<InsightStretch> = None;
        let keep_longest = |best: &mut Option<InsightStretch>, candidate: &InsightStretch| {
            if best
                .as_ref()
                .is_none_or(|best| candidate.milliseconds > best.milliseconds)
            {
                *best = Some(candidate.clone());
            }
        };

        for slice in &slices {
            let duration = slice.end - slice.start;
            if duration <= 0 {
                continue;
            }
            if slice.app.idle {
                idle += duration;
                continue;
            }
            effective += duration;
            add_interval(&mut day_points, slice.start, slice.end);
            add_by_local_hour(&mut heatmap, &mut hourly, slice.start, slice.end);
            let first_day = day_points.partition_point(|day| day.ended_at <= slice.start);
            for index in first_day..day_points.len() {
                let day = &day_points[index];
                if day.started_at >= slice.end {
                    break;
                }
                let from = slice.start.max(day.started_at);
                let until = slice.end.min(day.ended_at);
                firsts[index] = Some(firsts[index].map_or(from, |value: i64| value.min(from)));
                lasts[index] = Some(lasts[index].map_or(until, |value: i64| value.max(until)));
            }

            let target = slice.app.target.as_str();
            if previous_target.is_some_and(|previous| previous != target) {
                switches += 1;
            }
            previous_target = Some(target);

            match stretch.as_mut() {
                Some(current) if slice.start - current.ended_at <= STRETCH_GAP_MILLISECONDS => {
                    current.ended_at = current.ended_at.max(slice.end);
                    current.milliseconds += duration;
                }
                _ => {
                    if let Some(finished) = &stretch {
                        keep_longest(&mut longest_stretch, finished);
                    }
                    stretch = Some(InsightStretch {
                        started_at: slice.start,
                        ended_at: slice.end,
                        milliseconds: duration,
                        name: None,
                        target: None,
                    });
                }
            }
            match session.as_mut() {
                Some(current)
                    if current.target.as_deref() == Some(target)
                        && slice.start - current.ended_at <= STRETCH_GAP_MILLISECONDS =>
                {
                    current.ended_at = current.ended_at.max(slice.end);
                    current.milliseconds += duration;
                }
                _ => {
                    if let Some(finished) = &session {
                        keep_longest(&mut longest_session, finished);
                    }
                    session = Some(InsightStretch {
                        started_at: slice.start,
                        ended_at: slice.end,
                        milliseconds: duration,
                        name: Some(slice.app.name.clone()),
                        target: Some(target.to_string()),
                    });
                }
            }
        }
        if let Some(finished) = &stretch {
            keep_longest(&mut longest_stretch, finished);
        }
        if let Some(finished) = &session {
            keep_longest(&mut longest_session, finished);
        }

        let days = day_points
            .iter()
            .enumerate()
            .map(|(index, day)| InsightDay {
                started_at: day.started_at,
                ended_at: day.ended_at,
                label: Local
                    .timestamp_millis_opt(day.started_at)
                    .single()
                    .map(|moment| moment.format("%Y-%m-%d").to_string())
                    .unwrap_or_default(),
                milliseconds: day.milliseconds,
                first_activity_at: firsts[index],
                last_activity_at: lasts[index],
            })
            .collect::<Vec<_>>();
        let active = days
            .iter()
            .filter(|day| day.milliseconds >= ACTIVE_DAY_MILLISECONDS)
            .collect::<Vec<_>>();
        let first_minutes = active
            .iter()
            .filter_map(|day| {
                day.first_activity_at
                    .map(|time| minute_of_day(time, day.started_at, day_start_hour))
            })
            .collect::<Vec<_>>();
        let last_minutes = active
            .iter()
            .filter_map(|day| {
                day.last_activity_at
                    .map(|time| minute_of_day(time, day.started_at, day_start_hour))
            })
            .collect::<Vec<_>>();
        let peak_hour = hourly
            .iter()
            .enumerate()
            .filter(|(_, value)| **value > 0)
            .max_by_key(|(hour, value)| (**value, std::cmp::Reverse(*hour)))
            .map(|(hour, _)| hour as u32);

        let elapsed = (now.min(end) - start).max(0);
        let previous_start = start - (end - start);
        let previous_end = previous_start + elapsed;
        let previous_effective_milliseconds = if elapsed > 0 {
            self.statistic_slices(&query(previous_start, previous_end))?
                .into_iter()
                .filter(|slice| !slice.excluded && !slice.app.idle)
                .map(|slice| (slice.end - slice.start).max(0))
                .sum()
        } else {
            0
        };
        let active_hours = effective as f64 / 3_600_000.0;
        Ok(UsageInsights {
            start,
            end,
            effective_milliseconds: effective,
            idle_milliseconds: idle,
            previous_start,
            previous_end,
            previous_effective_milliseconds,
            day_count: days.len(),
            active_days: active.len(),
            heatmap,
            hourly,
            peak_hour,
            longest_stretch,
            longest_session,
            switches,
            switches_per_hour: if active_hours >= 0.05 {
                switches as f64 / active_hours
            } else {
                0.0
            },
            average_first_minute: average(&first_minutes),
            average_last_minute: average(&last_minutes),
            days,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_hours_split_at_boundaries() {
        let mut heatmap = vec![vec![0; 24]; 7];
        let mut hourly = vec![0; 24];
        let start = Local
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2026, 11, 9)
                    .unwrap()
                    .and_hms_opt(9, 30, 0)
                    .unwrap(),
            )
            .single()
            .unwrap()
            .timestamp_millis();
        add_by_local_hour(&mut heatmap, &mut hourly, start, start + 90 * 60_000);
        // 2026-11-09 is a Monday: 30 minutes at 09:00 and a full hour at 10:00.
        assert_eq!(heatmap[0][9], 30 * 60_000);
        assert_eq!(heatmap[0][10], 60 * 60_000);
        assert_eq!(hourly.iter().sum::<i64>(), 90 * 60_000);
    }

    #[test]
    #[ignore = "measures a copied database named by KEDU_INSIGHTS_DB"]
    fn insights_performance_on_copy() {
        let Ok(source) = std::env::var("KEDU_INSIGHTS_DB") else {
            return;
        };
        // Work on a private copy; the given file is never opened for writing.
        let directory = std::env::temp_dir().join(format!("kedu-insights-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(".kedu-test-data"), "").unwrap();
        let copy = directory.join("kedu.sqlite");
        std::fs::copy(&source, &copy).unwrap();
        let storage = Storage::new(copy).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        for days in [1_i64, 28, 84, 364] {
            let started = std::time::Instant::now();
            let result = storage
                .query_insights(now - days * 86_400_000, now, now)
                .unwrap();
            eprintln!(
                "days={days} elapsed_ms={} effective_h={:.1} active_days={}",
                started.elapsed().as_millis(),
                result.effective_milliseconds as f64 / 3_600_000.0,
                result.active_days
            );
        }
        drop(storage);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn averages_and_minutes_are_stable() {
        assert_eq!(average(&[]), None);
        assert_eq!(average(&[60, 120]), Some(90));
        assert_eq!(minute_of_day(3_600_000, 0, 4), 4 * 60 + 60);
    }
}
