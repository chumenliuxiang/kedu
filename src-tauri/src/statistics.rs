//! One foreground partition feeds every statistics view. Raw rows remain untouched.
use super::*;
use crate::domain::{ActivityDetails, StatisticsExclusion, StatisticsQuery};
use chrono::Datelike;

#[path = "insights.rs"]
mod insights;
pub use insights::UsageInsights;

#[derive(Clone)]
struct Segment {
    id: i64,
    start: i64,
    end: i64,
    name: String,
    target: String,
    title: String,
    search_name: String,
    search_title: String,
    idle: bool,
    site: String,
    page_key: String,
    visit_id: String,
    browser_target: String,
}

#[derive(Clone)]
struct Slice {
    app: Segment,
    web: Option<Segment>,
    start: i64,
    end: i64,
    category: String,
    excluded: bool,
    manual: bool,
    annotation_id: Option<i64>,
}

struct CategoryRule {
    kind: String,
    pattern: String,
    category: String,
    updated: i64,
    id: i64,
    exact: bool,
}

type CategoryMatchKey<'a> = (&'a str, &'a str, &'a str, Option<(&'a str, &'a str)>);

impl CategoryRule {
    fn priority(&self) -> (u8, i64, i64) {
        let priority = match self.kind.as_str() {
            "title" => 3,
            "website" => 2,
            _ => 1,
        };
        (priority, self.updated, self.id)
    }

    fn matches(&self, app: &Segment, web: Option<&Segment>) -> bool {
        let pattern = &self.pattern;
        match self.kind.as_str() {
            "application" if self.exact => {
                app.target == crate::app_identity::canonical_executable(pattern)
            }
            "application" => crate::app_identity::matches(&app.target, &app.search_name, pattern),
            "website" => web.is_some_and(|web| {
                if self.exact {
                    web.target == *pattern || web.site == *pattern
                } else {
                    domain_matches(&web.target, pattern)
                }
            }),
            "title" => {
                !pattern.is_empty()
                    && (app.search_title.contains(pattern)
                        || web.is_some_and(|web| web.search_title.contains(pattern)))
            }
            _ => false,
        }
    }
}

fn application_target(path: &str, name: &str) -> String {
    let value = if path.trim().is_empty() { name } else { path };
    crate::app_identity::canonical_executable(value)
}

fn browser(target: &str) -> bool {
    matches!(
        target,
        "msedge.exe"
            | "chrome.exe"
            | "firefox.exe"
            | "brave.exe"
            | "opera.exe"
            | "vivaldi.exe"
            | "browser.exe"
            | "quark.exe"
            | "arc.exe"
    )
}

fn color(target: &str) -> String {
    let palette = [
        // Validated categorical order; the entity hash keeps a target's colour stable.
        "#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948",
    ];
    let hash = target.bytes().fold(2166136261u32, |hash, byte| {
        (hash ^ byte as u32).wrapping_mul(16777619)
    });
    palette[hash as usize % palette.len()].into()
}

fn target_value(kind: &str, target: &str) -> Result<String, String> {
    if target.trim().is_empty() {
        return Err("请选择统计目标".into());
    }
    match kind {
        "application" => Ok(application_target(target, target)),
        "website" => Ok(normalize_domain(target)),
        "category" => Ok(target.trim().to_string()),
        _ => Err("未知的统计目标类型".into()),
    }
}

fn target_matches(slice: &Slice, kind: Option<&str>, target: Option<&str>) -> bool {
    match (kind, target) {
        (Some("application"), Some(value)) => slice.app.target == value,
        (Some("website"), Some(value)) => slice
            .web
            .as_ref()
            .is_some_and(|web| domain_matches(&web.target, value) || web.site == value),
        (Some("category"), Some(value)) => slice.category == value,
        _ => true,
    }
}

fn local_timestamp(date: NaiveDate, hour: u32) -> Result<i64, String> {
    let naive = date.and_hms_opt(hour, 0, 0).ok_or("无效的日期边界")?;
    // A skipped wall-clock hour is advanced to the next valid hour.
    for shift in 0..=3 {
        if let Some(value) = Local
            .from_local_datetime(&(naive + chrono::Duration::hours(shift)))
            .earliest()
        {
            return Ok(value.timestamp_millis());
        }
    }
    Err("无法解析本地日期边界".into())
}

fn buckets(query: &StatisticsQuery, day_start_hour: u32) -> Result<Vec<TrendPoint>, String> {
    let mut result = Vec::new();
    let mut start = query.start;
    while start < query.end {
        let date = Local
            .timestamp_millis_opt(start)
            .single()
            .ok_or("日期超出支持范围")?;
        let end = match query.bucket.as_str() {
            "hour" => {
                start + 3_600_000
                    - (date.minute() as i64 * 60_000
                        + date.second() as i64 * 1000
                        + date.timestamp_subsec_millis() as i64)
            }
            "day" => {
                let mut boundary = local_timestamp(date.date_naive(), day_start_hour)?;
                if boundary <= start {
                    boundary = local_timestamp(
                        date.date_naive().succ_opt().ok_or("日期超出支持范围")?,
                        day_start_hour,
                    )?;
                }
                boundary
            }
            "month" => {
                let first =
                    NaiveDate::from_ymd_opt(date.year(), date.month(), 1).ok_or("无效月份")?;
                let next = first
                    .checked_add_months(chrono::Months::new(1))
                    .ok_or("日期超出支持范围")?;
                local_timestamp(next, day_start_hour)?
            }
            _ => return Err("统计粒度必须是 hour、day 或 month".into()),
        }
        .min(query.end);
        if end <= start {
            return Err("无效的统计时间边界".into());
        }
        let format = match query.bucket.as_str() {
            "hour" => "%m-%d %H:%M",
            "day" => "%m-%d",
            _ => "%Y-%m",
        };
        result.push(TrendPoint {
            started_at: start,
            ended_at: end,
            label: date.format(format).to_string(),
            seconds: 0,
            milliseconds: 0,
        });
        start = end;
    }
    Ok(result)
}

fn add_interval(points: &mut [TrendPoint], start: i64, end: i64) {
    let index = points.partition_point(|point| point.ended_at <= start);
    for point in points
        .iter_mut()
        .skip(index)
        .take_while(|point| point.started_at < end)
    {
        point.milliseconds += (point.ended_at.min(end) - point.started_at.max(start)).max(0);
        point.seconds = point.milliseconds / 1000;
    }
}

fn usage(map: BTreeMap<String, (String, i64)>, total: i64) -> Vec<AppUsage> {
    let mut rows = map
        .into_iter()
        .map(|(target, (app_name, milliseconds))| AppUsage {
            color: color(&target),
            target,
            app_name,
            milliseconds,
            seconds: milliseconds / 1000,
            percent: if total > 0 {
                milliseconds as f64 / total as f64 * 100.0
            } else {
                0.0
            },
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .milliseconds
            .cmp(&left.milliseconds)
            .then(left.target.cmp(&right.target))
    });
    rows
}

fn timeline_item(slice: &Slice, website: bool) -> TimelineItem {
    let source = if website {
        slice.web.as_ref().unwrap_or(&slice.app)
    } else {
        &slice.app
    };
    TimelineItem {
        id: source.id,
        app_name: source.name.clone(),
        title: source.title.clone(),
        started_at: slice.start,
        ended_at: slice.end,
        milliseconds: slice.end - slice.start,
        seconds: (slice.end - slice.start) / 1000,
        idle: slice.app.idle,
        source_kind: if website { "website" } else { "application" }.into(),
        target: if website {
            source.site.clone()
        } else {
            source.target.clone()
        },
        domain: slice.web.as_ref().map(|web| web.target.clone()),
        page_key: slice.web.as_ref().map(|web| web.page_key.clone()),
        visit_id: if website {
            slice.web.as_ref().map(|web| web.visit_id.clone())
        } else {
            None
        },
        source_ids: if website { Some(vec![source.id]) } else { None },
        category: slice.category.clone(),
        color: if slice.app.idle {
            "#cbd0d5".into()
        } else {
            color(&source.target)
        },
    }
}

fn joined_timeline(slices: &[Slice], website: bool) -> Vec<TimelineItem> {
    let mut result: Vec<TimelineItem> = Vec::new();
    for slice in slices {
        let item = timeline_item(slice, website);
        if let Some(last) = result.last_mut() {
            if (last.id == item.id
                || (website
                    && last.visit_id.is_some()
                    && last.visit_id == item.visit_id
                    && last.page_key == item.page_key))
                && last.ended_at == item.started_at
                && last.category == item.category
                && (!website || last.domain == item.domain)
                && last.idle == item.idle
            {
                last.ended_at = item.ended_at;
                last.milliseconds += item.milliseconds;
                last.seconds = last.milliseconds / 1000;
                if website {
                    last.title = item.title;
                    if let Some(ids) = last.source_ids.as_mut() {
                        if !ids.contains(&item.id) {
                            ids.push(item.id);
                        }
                    }
                }
                continue;
            }
        }
        result.push(item);
    }
    result
}

impl Storage {
    pub fn database_path(&self) -> &Path {
        &self.path
    }

    pub fn persistent_autostart(&self) -> Result<Option<bool>, String> {
        self.connection()?
            .query_row(
                "SELECT value FROM app_settings WHERE key='autostart_enabled'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map(|value| value.map(|value| value == "true"))
            .map_err(|error| error.to_string())
    }

    pub fn set_persistent_autostart(&self, enabled: bool) -> Result<(), String> {
        self.connection()?.execute("INSERT INTO app_settings(key,value) VALUES('autostart_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [enabled.to_string()])
            .map(|_| ()).map_err(|error| error.to_string())
    }

    fn statistic_slices(&self, query: &StatisticsQuery) -> Result<Vec<Slice>, String> {
        let connection = self.connection()?;
        connection
            .execute_batch("BEGIN DEFERRED TRANSACTION")
            .map_err(|error| error.to_string())?;
        Self::statistic_slices_on(&connection, query)
    }

    fn statistic_slices_on(
        connection: &Connection,
        query: &StatisticsQuery,
    ) -> Result<Vec<Slice>, String> {
        if query.end <= query.start {
            return Err("统计结束时间必须晚于开始时间".into());
        }
        if query.end.saturating_sub(query.start) > 10 * 366 * 86_400_000 {
            return Err("单次统计范围不能超过十年".into());
        }
        if query.target_kind.is_some() != query.target.is_some() {
            return Err("统计目标缺少类型或名称".into());
        }
        let mut statement = connection.prepare("SELECT id, started_at, ended_at, app_name, process_path, title, idle FROM activity_segments WHERE ended_at>?1 AND started_at<?2 AND ended_at>started_at ORDER BY started_at,id").map_err(|error| error.to_string())?;
        let apps = statement
            .query_map(params![query.start, query.end], |row| {
                let name: String = row.get(3)?;
                let path: String = row.get(4)?;
                let title: String = row.get(5)?;
                Ok(Segment {
                    id: row.get(0)?,
                    start: row.get(1)?,
                    end: row.get(2)?,
                    target: application_target(&path, &name),
                    search_name: name.to_lowercase(),
                    search_title: title.to_lowercase(),
                    name: crate::app_identity::display_name(&path, &name),
                    title,
                    idle: row.get::<_, i64>(6)? != 0,
                    site: String::new(),
                    page_key: String::new(),
                    visit_id: String::new(),
                    browser_target: String::new(),
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let mut statement = connection.prepare("SELECT id, started_at, ended_at, domain, title, idle, page_key, visit_id, browser_target FROM web_activity_segments WHERE ended_at>?1 AND started_at<?2 AND ended_at>started_at ORDER BY started_at,id").map_err(|error| error.to_string())?;
        let sites = statement
            .query_map(params![query.start, query.end], |row| {
                let name: String = row.get(3)?;
                let title: String = row.get(4)?;
                let (site, display_name) = web_recording::site_identity(&name);
                let page_key: String = row.get(6)?;
                let visit_id: String = row.get(7)?;
                let id: i64 = row.get(0)?;
                Ok(Segment {
                    id,
                    start: row.get(1)?,
                    end: row.get(2)?,
                    target: normalize_domain(&name),
                    search_name: name.to_lowercase(),
                    search_title: title.to_lowercase(),
                    name: display_name,
                    title,
                    idle: row.get::<_, i64>(5)? != 0,
                    site,
                    // Older rows have no reliable page identity. Keep them visibly
                    // separate instead of inventing a video/path from their title.
                    page_key: if page_key.is_empty() {
                        format!("legacy:{id}")
                    } else {
                        page_key
                    },
                    visit_id: if visit_id.is_empty() {
                        format!("legacy:{id}")
                    } else {
                        visit_id
                    },
                    browser_target: row.get(8)?,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let mut statement = connection.prepare("SELECT target_kind,target_pattern,category,updated_at,id FROM classification_rules WHERE enabled=1 UNION ALL SELECT target_kind,target,category,updated_at,0 FROM statistics_defaults").map_err(|error| error.to_string())?;
        let mut rules = statement
            .query_map([], |row| {
                let id = row.get(4)?;
                Ok(CategoryRule {
                    kind: row.get(0)?,
                    pattern: row.get::<_, String>(1)?.trim().to_lowercase(),
                    category: row.get(2)?,
                    updated: row.get(3)?,
                    id,
                    exact: id == 0,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        // The former max_by_key selected the last matching row for equal keys.
        // Preserve that tie-break while evaluating the winning rule first.
        rules.reverse();
        rules.sort_by_key(|rule| std::cmp::Reverse(rule.priority()));
        let mut statement = connection
            .prepare("SELECT source_kind,source_id,category,updated_at FROM statistics_overrides")
            .map_err(|error| error.to_string())?;
        let overrides = statement
            .query_map([], |row| {
                Ok((
                    (row.get::<_, String>(0)?, row.get::<_, i64>(1)?),
                    (row.get::<_, String>(2)?, row.get::<_, i64>(3)?),
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare("SELECT target_kind,target FROM statistics_exclusions")
            .map_err(|error| error.to_string())?;
        let exclusions = statement
            .query_map([], |row| {
                let kind: String = row.get(0)?;
                let target: String = row.get(1)?;
                Ok((
                    kind.clone(),
                    if kind == "application" {
                        crate::app_identity::canonical_executable(&target)
                    } else {
                        target
                    },
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(|error| error.to_string())?;

        let annotations = timeline_annotations::load(connection, query.start, query.end)?;
        // End events sort before start events at an equal timestamp: intervals are [start,end).
        let mut events = Vec::with_capacity((apps.len() + sites.len()) * 2);
        for (kind, rows) in [(0, &apps), (1, &sites)] {
            for (index, row) in rows.iter().enumerate() {
                events.push((row.start.max(query.start), 1, kind, index));
                events.push((row.end.min(query.end), 0, kind, index));
            }
        }
        for (index, annotation) in annotations.iter().enumerate() {
            events.push((annotation.start.max(query.start), 1, 2, index));
            events.push((annotation.end.min(query.end), 0, 2, index));
        }
        events.sort_unstable();
        let mut active_annotations = BTreeSet::new();
        let mut active_apps = BTreeSet::new();
        let mut active_sites = BTreeSet::new();
        let mut slices = Vec::new();
        // Many foreground segments revisit the same window. Rules depend on
        // these strings, not record IDs; manual per-record corrections stay out
        // of this bounded, query-local cache and retain their precedence.
        let mut category_cache = BTreeMap::<CategoryMatchKey<'_>, String>::new();
        for (position, &(time, action, kind, index)) in events.iter().enumerate() {
            if kind == 2 {
                let annotation = &annotations[index];
                let key = (annotation.revision, annotation.id, index);
                if action == 1 {
                    active_annotations.insert(key);
                } else {
                    active_annotations.remove(&key);
                }
            } else {
                let rows = if kind == 0 { &apps } else { &sites };
                let active = if kind == 0 {
                    &mut active_apps
                } else {
                    &mut active_sites
                };
                let key = (rows[index].start, rows[index].id, index);
                if action == 1 {
                    active.insert(key);
                } else {
                    active.remove(&key);
                }
            }
            let Some(&(next, _, _, _)) = events.get(position + 1) else {
                continue;
            };
            if next <= time {
                continue;
            }
            let Some(&(_, _, app_index)) = active_apps.last() else {
                continue;
            };
            let app = &apps[app_index];
            let web = if browser(&app.target) && !app.idle {
                active_sites
                    .iter()
                    .rev()
                    .map(|&(_, _, index)| &sites[index])
                    .find(|web| !web.idle && web.browser_target == app.target)
            } else {
                None
            };
            let manual = [
                overrides.get(&("application".into(), app.id)),
                web.and_then(|web| overrides.get(&("website".into(), web.id))),
            ]
            .into_iter()
            .flatten()
            .max_by_key(|(_, updated)| *updated);
            let annotation = active_annotations
                .last()
                .map(|&(_, _, index)| &annotations[index])
                .filter(|annotation| {
                    manual.is_none_or(|(_, revision)| annotation.revision > *revision)
                });
            let explicit = match annotation {
                Some(annotation) => annotation.category.as_ref(),
                None => manual.map(|(category, _)| category),
            };
            let category = if let Some(category) = explicit {
                category.clone()
            } else {
                let key = (
                    app.target.as_str(),
                    app.search_name.as_str(),
                    app.search_title.as_str(),
                    web.map(|web| (web.target.as_str(), web.search_title.as_str())),
                );
                if let Some(category) = category_cache.get(&key) {
                    category.clone()
                } else {
                    let category = rules
                        .iter()
                        .find(|rule| rule.matches(app, web))
                        .map(|rule| rule.category.clone())
                        .unwrap_or_else(|| "未分类".into());
                    if category_cache.len() < 4096 {
                        category_cache.insert(key, category.clone());
                    }
                    category
                }
            };
            let excluded = exclusions.contains(&("application".into(), app.target.clone()))
                || web.is_some_and(|web| {
                    exclusions.iter().any(|(kind, target)| {
                        kind == "website"
                            && (domain_matches(&web.target, target) || web.site == *target)
                    })
                });
            slices.push(Slice {
                app: app.clone(),
                web: web.cloned(),
                start: time,
                end: next,
                category,
                excluded,
                manual: explicit.is_some(),
                annotation_id: annotation.map(|a| a.id),
            });
        }
        if let Some(page_key) = &query.page_key {
            if query.target_kind.as_deref() != Some("website") {
                return Err("页面筛选需要先选择网站".into());
            }
            slices.retain(|slice| {
                slice
                    .web
                    .as_ref()
                    .is_some_and(|web| web.page_key == *page_key)
            });
        }
        Ok(slices)
    }

    pub(super) fn timeline_items_on(
        connection: &Connection,
        query: &StatisticsQuery,
    ) -> Result<Vec<TimelineItem>, String> {
        let slices = Self::statistic_slices_on(connection, query)?
            .into_iter()
            .filter(|s| !s.excluded)
            .collect::<Vec<_>>();
        Ok(joined_timeline(&slices, false))
    }
    pub(super) fn timeline_candidates_on(
        connection: &Connection,
        query: &StatisticsQuery,
    ) -> Result<Vec<crate::timeline_models::TimelineCandidate>, String> {
        Ok(Self::statistic_slices_on(connection, query)?
            .into_iter()
            .filter(|s| !s.excluded && !s.app.idle)
            .map(|s| crate::timeline_models::TimelineCandidate {
                start: s.start,
                end: s.end,
                category: s.category,
                manual: s.manual,
                annotation_id: s.annotation_id,
            })
            .collect())
    }

    pub(super) fn classification_usage_on(
        connection: &Connection,
        now: i64,
    ) -> Result<BTreeMap<String, i64>, String> {
        let query = StatisticsQuery {
            start: now - 30 * 86_400_000,
            end: now,
            bucket: "day".into(),
            target_kind: None,
            target: None,
            offset: None,
            limit: None,
            search: None,
            include_excluded: None,
            page_key: None,
        };
        let mut result = BTreeMap::new();
        for slice in Self::statistic_slices_on(connection, &query)? {
            if slice.excluded || slice.app.idle {
                continue;
            }
            let duration = slice.end - slice.start;
            *result
                .entry(format!("application:{}", slice.app.target))
                .or_insert(0) += duration;
            if let Some(web) = slice.web {
                *result.entry(format!("website:{}", web.site)).or_insert(0) += duration;
            }
        }
        Ok(result)
    }

    pub fn query_statistics(&self, query: StatisticsQuery) -> Result<ActivityStatistics, String> {
        self.query_statistics_with_options(query, true, true)
    }

    pub fn query_statistics_with_options(
        &self,
        query: StatisticsQuery,
        include_timeline: bool,
        include_trend_series: bool,
    ) -> Result<ActivityStatistics, String> {
        let target = query
            .target_kind
            .as_deref()
            .zip(query.target.as_deref())
            .map(|(kind, target)| target_value(kind, target))
            .transpose()?;
        let slices = self
            .statistic_slices(&query)?
            .into_iter()
            .filter(|slice| {
                !slice.excluded
                    && target_matches(slice, query.target_kind.as_deref(), target.as_deref())
            })
            .collect::<Vec<_>>();
        let empty_trend = buckets(&query, self.settings()?.day_start_hour as u32)?;
        let mut trend = empty_trend.clone();
        let mut apps = BTreeMap::<String, (String, i64)>::new();
        let mut sites = BTreeMap::<String, (String, i64)>::new();
        let mut categories = BTreeMap::<String, (String, i64)>::new();
        let mut effective_milliseconds = 0;
        let mut tracked_milliseconds = 0;
        for slice in &slices {
            let duration = slice.end - slice.start;
            tracked_milliseconds += duration;
            if slice.app.idle {
                continue;
            }
            effective_milliseconds += duration;
            apps.entry(slice.app.target.clone())
                .or_insert((slice.app.name.clone(), 0))
                .1 += duration;
            categories
                .entry(slice.category.clone())
                .or_insert((slice.category.clone(), 0))
                .1 += duration;
            if let Some(site) = &slice.web {
                sites
                    .entry(site.site.clone())
                    .or_insert((site.name.clone(), 0))
                    .1 += duration;
            }
            add_interval(&mut trend, slice.start, slice.end);
        }
        let app_usage = usage(apps, effective_milliseconds);
        let site_usage = usage(sites, effective_milliseconds);
        let mut category_usage = usage(categories, effective_milliseconds);
        let category_definitions = category_management::list_categories(&self.connection()?)?;
        for item in &mut category_usage {
            if let Some(category) = category_definitions
                .iter()
                .find(|c| c.name == item.app_name)
            {
                item.color = category.color.clone();
            }
        }
        // Small compatibility series for old clients. New clients query any selected target.
        let mut trend_series = Vec::new();
        let mut series_indices = BTreeMap::new();
        if include_trend_series {
            for (kind, rows) in [
                ("application", &app_usage),
                ("website", &site_usage),
                ("category", &category_usage),
            ] {
                for row in rows.iter().take(5) {
                    series_indices.insert((kind, row.target.as_str()), trend_series.len());
                    trend_series.push(TrendSeries {
                        kind: kind.into(),
                        name: row.app_name.clone(),
                        color: row.color.clone(),
                        points: empty_trend.clone(),
                    });
                }
            }
            for slice in slices.iter().filter(|slice| !slice.app.idle) {
                for key in [
                    Some(("application", slice.app.target.as_str())),
                    Some(("category", slice.category.as_str())),
                    slice.web.as_ref().map(|web| ("website", web.site.as_str())),
                ]
                .into_iter()
                .flatten()
                {
                    if let Some(&index) = series_indices.get(&key) {
                        add_interval(&mut trend_series[index].points, slice.start, slice.end);
                    }
                }
            }
        }
        let records_count = slices
            .iter()
            .enumerate()
            .filter(|(index, slice)| {
                *index == 0 || {
                    let last = &slices[*index - 1];
                    last.app.id != slice.app.id
                        || last.end != slice.start
                        || last.category != slice.category
                        || last.app.idle != slice.app.idle
                }
            })
            .count();
        let timeline = if include_timeline {
            joined_timeline(&slices, false)
        } else {
            Vec::new()
        };
        Ok(ActivityStatistics {
            start: query.start,
            end: query.end,
            effective_seconds: effective_milliseconds / 1000,
            tracked_seconds: tracked_milliseconds / 1000,
            effective_milliseconds,
            tracked_milliseconds,
            records_count,
            app_usage,
            site_usage,
            category_usage,
            trend,
            trend_series,
            timeline,
        })
    }

    pub fn query_activity_details(
        &self,
        query: StatisticsQuery,
    ) -> Result<ActivityDetails, String> {
        let target = query
            .target_kind
            .as_deref()
            .zip(query.target.as_deref())
            .map(|(kind, target)| target_value(kind, target))
            .transpose()?;
        let search = query.search.as_deref().unwrap_or("").trim().to_lowercase();
        let slices = self
            .statistic_slices(&query)?
            .into_iter()
            .filter(|slice| {
                (query.include_excluded.unwrap_or(false) || !slice.excluded)
                    && target_matches(slice, query.target_kind.as_deref(), target.as_deref())
                    && (search.is_empty()
                        || slice.app.search_name.contains(&search)
                        || slice.app.search_title.contains(&search)
                        || slice.web.as_ref().is_some_and(|web| {
                            web.search_title.contains(&search) || web.target.contains(&search)
                        }))
            })
            .collect::<Vec<_>>();
        let mut items = joined_timeline(&slices, query.target_kind.as_deref() == Some("website"));
        items.reverse();
        let total = items.len();
        let offset = query.offset.unwrap_or(0);
        let limit = query.limit.unwrap_or(50).clamp(1, 500);
        Ok(ActivityDetails {
            items: items.into_iter().skip(offset).take(limit).collect(),
            total,
            offset,
            limit,
        })
    }

    pub fn query_ranking(
        &self,
        query: StatisticsQuery,
        kind: &str,
        offset: usize,
        limit: usize,
    ) -> Result<crate::domain::RankingPage, String> {
        let statistics = self.query_statistics_with_options(query, false, false)?;
        let rows = match kind {
            "application" => statistics.app_usage,
            "website" => statistics.site_usage,
            "category" => statistics.category_usage,
            _ => return Err("排行类型必须是应用、网站或分类".into()),
        };
        let total = rows.len();
        let limit = limit.clamp(1, 200);
        Ok(crate::domain::RankingPage {
            items: rows.into_iter().skip(offset).take(limit).collect(),
            total,
            offset,
            limit,
        })
    }

    pub fn query_website_pages(
        &self,
        mut query: StatisticsQuery,
        site: &str,
        offset: usize,
        limit: usize,
        search: &str,
    ) -> Result<crate::domain::WebsitePages, String> {
        let site = target_value("website", site)?;
        query.target_kind = Some("website".into());
        query.target = Some(site.clone());
        query.page_key = None;
        let search = search.trim().to_lowercase();
        let mut pages =
            BTreeMap::<String, (crate::domain::WebsitePageUsage, BTreeSet<String>, bool)>::new();
        for slice in self.statistic_slices(&query)? {
            if slice.app.idle
                || (slice.excluded && !query.include_excluded.unwrap_or(false))
                || !target_matches(&slice, Some("website"), Some(&site))
            {
                continue;
            }
            let Some(web) = slice.web else {
                continue;
            };
            let matches_search = search.is_empty()
                || web.search_title.contains(&search)
                || web.page_key.to_lowercase().contains(&search);
            let (page, visits, matches_any) =
                pages.entry(web.page_key.clone()).or_insert_with(|| {
                    (
                        crate::domain::WebsitePageUsage {
                            site: web.site.clone(),
                            page_key: web.page_key.clone(),
                            title: web.title.clone(),
                            domain: web.target.clone(),
                            milliseconds: 0,
                            seconds: 0,
                            visits: 0,
                            last_seen_at: slice.end,
                        },
                        BTreeSet::new(),
                        false,
                    )
                });
            page.milliseconds += slice.end - slice.start;
            *matches_any |= matches_search;
            if slice.end >= page.last_seen_at {
                page.title = web.title;
                page.domain = web.target;
                page.last_seen_at = slice.end;
            }
            visits.insert(web.visit_id);
        }
        let mut items = pages
            .into_values()
            .filter(|(_, _, matches)| *matches)
            .map(|(mut page, visits, _)| {
                page.seconds = page.milliseconds / 1000;
                page.visits = visits.len();
                page
            })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| {
            b.milliseconds
                .cmp(&a.milliseconds)
                .then(b.last_seen_at.cmp(&a.last_seen_at))
                .then(a.page_key.cmp(&b.page_key))
        });
        let total = items.len();
        let limit = limit.clamp(1, 200);
        Ok(crate::domain::WebsitePages {
            items: items.into_iter().skip(offset).take(limit).collect(),
            total,
            offset,
            limit,
        })
    }

    pub fn set_default_category(
        &self,
        kind: &str,
        target: &str,
        category: &str,
    ) -> Result<(), String> {
        if !matches!(kind, "application" | "website") {
            return Err("只能设置应用或网站默认分类".into());
        }
        let target = target_value(kind, target)?;
        let mut database = self.connection()?;
        let connection = database
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if kind == "application" && target == "steam.exe" {
            // Legacy helper defaults match the unified client when read. Clear
            // them together when replacing/removing its default, otherwise an
            // old helper category would reappear after the canonical row is gone.
            connection
                .execute(
                    "DELETE FROM statistics_defaults WHERE target_kind='application'
                 AND lower(target) IN ('steam','steam.exe','steamwebhelper','steamwebhelper.exe')",
                    [],
                )
                .map_err(|error| error.to_string())?;
        }
        if category.trim().is_empty() {
            connection
                .execute(
                    "DELETE FROM statistics_defaults WHERE target_kind=?1 AND target=?2",
                    params![kind, target],
                )
                .map_err(|error| error.to_string())?;
        } else {
            connection.execute("INSERT INTO statistics_defaults(target_kind,target,category,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(target_kind,target) DO UPDATE SET category=excluded.category,updated_at=excluded.updated_at",params![kind,target,category.trim(),category_management::next_revision(&connection)?]).map_err(|error| error.to_string())?;
        }
        connection.commit().map_err(|e| e.to_string())
    }

    pub fn set_activity_category(
        &self,
        kind: &str,
        id: i64,
        category: Option<&str>,
    ) -> Result<(), String> {
        self.set_activity_categories(kind, &[id], category)
    }

    pub fn set_activity_categories(
        &self,
        kind: &str,
        ids: &[i64],
        category: Option<&str>,
    ) -> Result<(), String> {
        if ids.is_empty() || ids.len() > 5000 {
            return Err("活动记录数量无效".into());
        }
        let table = match kind {
            "application" => "activity_segments",
            "website" => "web_activity_segments",
            _ => return Err("未知的活动来源".into()),
        };
        if kind != "website" && ids.len() != 1 {
            return Err("只有网站访问支持合并纠正".into());
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let ids = ids.iter().copied().collect::<BTreeSet<_>>();
        for id in &ids {
            let exists: bool = transaction
                .query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),
                    [id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if !exists {
                return Err("活动记录不存在".into());
            }
        }
        let updated_at = category_management::next_revision(&transaction)?;
        for id in ids {
            if let Some(category) = category.filter(|value| !value.trim().is_empty()) {
                transaction.execute("INSERT INTO statistics_overrides(source_kind,source_id,category,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(source_kind,source_id) DO UPDATE SET category=excluded.category,updated_at=excluded.updated_at",params![kind,id,category.trim(),updated_at]).map_err(|error| error.to_string())?;
            } else {
                transaction
                    .execute(
                        "DELETE FROM statistics_overrides WHERE source_kind=?1 AND source_id=?2",
                        params![kind, id],
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn set_statistics_excluded(
        &self,
        kind: &str,
        target: &str,
        excluded: bool,
    ) -> Result<(), String> {
        if !matches!(kind, "application" | "website") {
            return Err("只能排除应用或网站".into());
        }
        let target = target_value(kind, target)?;
        let mut database = self.connection()?;
        let connection = database
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        if kind == "application" && target == "steam.exe" {
            connection
                .execute(
                    "DELETE FROM statistics_exclusions WHERE target_kind='application'
                 AND lower(target) IN ('steam','steam.exe','steamwebhelper','steamwebhelper.exe')",
                    [],
                )
                .map_err(|error| error.to_string())?;
        }
        let sql = if excluded {
            "INSERT OR IGNORE INTO statistics_exclusions(target_kind,target) VALUES(?1,?2)"
        } else {
            "DELETE FROM statistics_exclusions WHERE target_kind=?1 AND target=?2"
        };
        connection
            .execute(sql, params![kind, target])
            .map_err(|error| error.to_string())?;
        connection.commit().map_err(|error| error.to_string())
    }

    pub fn list_statistics_exclusions(&self) -> Result<Vec<StatisticsExclusion>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT target_kind,target FROM statistics_exclusions ORDER BY target_kind,target",
            )
            .map_err(|error| error.to_string())?;
        let mut rows = statement
            .query_map([], |row| {
                let target: String = row.get(1)?;
                Ok(StatisticsExclusion {
                    target_kind: row.get(0)?,
                    display_name: target.clone(),
                    target,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        for row in &mut rows {
            if row.target_kind == "application" {
                row.target = crate::app_identity::canonical_executable(&row.target);
                if let Some(name) = connection.query_row("SELECT app_name FROM activity_segments WHERE lower(replace(process_path,'/','\\')) LIKE ?1 OR lower(app_name)=?2 ORDER BY ended_at DESC LIMIT 1",params![format!("%\\{}",row.target),row.target.trim_end_matches(".exe")],|row| row.get::<_,String>(0)).optional().map_err(|error| error.to_string())? { row.display_name = name; }
                row.display_name =
                    crate::app_identity::display_name(&row.target, &row.display_name);
            } else if row.target_kind == "website" {
                row.display_name = web_recording::site_identity(&row.target).1;
            }
        }
        Ok(rows
            .into_iter()
            .map(|row| ((row.target_kind.clone(), row.target.clone()), row))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture(PathBuf, Storage);
    impl Fixture {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join("kedu-tests").join(format!(
                "stats-{name}-{}-{}.sqlite",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let storage = Storage::new(path.clone()).unwrap();
            Self(path, storage)
        }
        fn insert_app(&self, start: i64, end: i64, name: &str, idle: bool) {
            self.1.connection().unwrap().execute(
                "INSERT INTO activity_segments(started_at,ended_at,app_name,process_path,title,idle) VALUES(?1,?2,?3,?4,?5,?6)",
                params![start,end,name,format!("C:\\{name}.exe"),format!("{name} title"),idle as i64],
            ).unwrap();
        }
        fn query(&self, start: i64, end: i64, bucket: &str) -> ActivityStatistics {
            self.1
                .query_statistics(StatisticsQuery {
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
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
            let _ = fs::remove_file(self.0.with_extension("sqlite-wal"));
            let _ = fs::remove_file(self.0.with_extension("sqlite-shm"));
        }
    }

    #[test]
    fn legacy_steam_alias_configuration_can_be_replaced_and_cleared() {
        let fixture = Fixture::new("steam-alias-config");
        fixture.insert_app(1_000, 2_000, "steamwebhelper", false);
        fixture.insert_app(2_000, 3_000, "Steam", false);
        fixture
            .1
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO statistics_defaults(target_kind,target,category,updated_at)
             VALUES('application','steamwebhelper.exe','旧类别',1)",
                [],
            )
            .unwrap();
        assert_eq!(
            fixture.query(0, 4_000, "hour").category_usage[0].app_name,
            "旧类别"
        );
        fixture
            .1
            .set_default_category("application", "steam.exe", "")
            .unwrap();
        assert_eq!(
            fixture.query(0, 4_000, "hour").category_usage[0].app_name,
            "未分类"
        );

        fixture
            .1
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO statistics_defaults(target_kind,target,category,updated_at)
             VALUES('application','steamwebhelper.exe','旧类别',1)",
                [],
            )
            .unwrap();
        fixture
            .1
            .set_default_category("application", "steamwebhelper.exe", "新类别")
            .unwrap();
        assert_eq!(
            fixture.query(0, 4_000, "hour").category_usage[0].app_name,
            "新类别"
        );
        let remaining: i64 = fixture
            .1
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM statistics_defaults WHERE target='steamwebhelper.exe'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);

        fixture
            .1
            .connection()
            .unwrap()
            .execute_batch(
                "INSERT INTO statistics_exclusions(target_kind,target) VALUES
             ('application','steamwebhelper.exe'),('application','Steam.exe');",
            )
            .unwrap();
        let exclusions = fixture.1.list_statistics_exclusions().unwrap();
        assert_eq!(exclusions.len(), 1);
        assert_eq!(exclusions[0].target, "steam.exe");
        assert_eq!(exclusions[0].display_name, "Steam");
        assert_eq!(fixture.query(0, 4_000, "hour").effective_seconds, 0);
        fixture
            .1
            .set_statistics_excluded("application", "steamwebhelper.exe", false)
            .unwrap();
        assert!(fixture.1.list_statistics_exclusions().unwrap().is_empty());
        assert_eq!(fixture.query(0, 4_000, "hour").effective_seconds, 2);
        let raw_count: i64 = fixture
            .1
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM activity_segments", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(raw_count, 2);
    }

    #[test]
    fn disjoint_buckets_never_create_negative_usage() {
        let fixture = Fixture::new("nonnegative");
        let day = Local
            .with_ymd_and_hms(2026, 10, 9, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        fixture.insert_app(
            day + 11 * 3_600_000,
            day + 11 * 3_600_000 + 187_000,
            "short",
            false,
        );
        let stats = fixture.query(day, day + 24 * 3_600_000, "hour");
        assert_eq!(stats.trend.len(), 24);
        assert!(stats.trend.iter().all(|point| point.milliseconds >= 0));
        assert_eq!(
            stats
                .trend
                .iter()
                .map(|point| point.milliseconds)
                .sum::<i64>(),
            187_000
        );
        assert_eq!(stats.effective_milliseconds, 187_000);
    }

    #[test]
    fn website_time_is_a_subset_and_category_precedence_is_deterministic() {
        let fixture = Fixture::new("website-subset");
        fixture
            .1
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO activity_segments
                 (started_at,ended_at,app_name,process_path,title,idle)
                 VALUES (0,60000,'Microsoft Edge','C:\\msedge.exe','Course window',0)",
                [],
            )
            .unwrap();
        fixture
            .1
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO web_activity_segments
                 (started_at,ended_at,domain,title,media_active,idle,idle_exempt)
                 VALUES (10000,50000,'learn.example','Course page',0,0,0)",
                [],
            )
            .unwrap();
        let connection = fixture.1.connection().unwrap();
        for (name, kind, pattern, category, updated) in [
            ("app", "application", "msedge.exe", "工作", 3),
            ("site", "website", "learn.example", "学习", 2),
            ("older title", "title", "course", "旧标题", 1),
            ("newer title", "title", "course", "课程", 4),
        ] {
            connection
                .execute(
                    "INSERT INTO classification_rules
                     (name,target_kind,target_pattern,category,enabled,updated_at)
                     VALUES (?1,?2,?3,?4,1,?5)",
                    params![name, kind, pattern, category, updated],
                )
                .unwrap();
        }
        drop(connection);
        let statistics = fixture.query(0, 60_000, "hour");
        assert_eq!(statistics.effective_milliseconds, 60_000);
        assert_eq!(statistics.app_usage[0].milliseconds, 60_000);
        assert_eq!(statistics.site_usage[0].milliseconds, 40_000);
        assert_eq!(statistics.category_usage[0].app_name, "课程");
        assert_eq!(statistics.category_usage[0].milliseconds, 60_000);
    }

    #[test]
    fn nonzero_day_boundary_splits_crossing_activity_without_loss() {
        let fixture = Fixture::new("day-boundary");
        fixture
            .1
            .connection()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO app_settings(key,value) VALUES('day_start_hour','4')",
                [],
            )
            .unwrap();
        let range_start = Local
            .with_ymd_and_hms(2026, 10, 9, 4, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let activity_start = Local
            .with_ymd_and_hms(2026, 10, 10, 3, 30, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let activity_end = Local
            .with_ymd_and_hms(2026, 10, 10, 4, 30, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let range_end = Local
            .with_ymd_and_hms(2026, 10, 11, 4, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        fixture.insert_app(activity_start, activity_end, "cross-day", false);
        let statistics = fixture.query(range_start, range_end, "day");
        assert_eq!(statistics.trend.len(), 2);
        assert_eq!(statistics.trend[0].milliseconds, 30 * 60_000);
        assert_eq!(statistics.trend[1].milliseconds, 30 * 60_000);
        assert_eq!(statistics.effective_milliseconds, 60 * 60_000);
    }

    #[test]
    fn totals_are_independent_of_ranking_length_and_categories_partition_time() {
        let fixture = Fixture::new("many-apps");
        for index in 0..12 {
            fixture.insert_app(
                index * 10_000,
                (index + 1) * 10_000,
                &format!("app{index}"),
                false,
            );
        }
        let stats = fixture.query(0, 120_000, "hour");
        assert_eq!(stats.app_usage.len(), 12);
        assert_eq!(stats.effective_milliseconds, 120_000);
        assert_eq!(
            stats
                .app_usage
                .iter()
                .map(|row| row.milliseconds)
                .sum::<i64>(),
            120_000
        );
        assert_eq!(
            stats
                .category_usage
                .iter()
                .map(|row| row.milliseconds)
                .sum::<i64>(),
            120_000
        );
        assert_eq!(stats.category_usage[0].app_name, "未分类");
        let page = fixture
            .1
            .query_ranking(
                StatisticsQuery {
                    start: 0,
                    end: 120_000,
                    bucket: "hour".into(),
                    target_kind: None,
                    target: None,
                    offset: None,
                    limit: None,
                    search: None,
                    include_excluded: None,
                    page_key: None,
                },
                "application",
                5,
                4,
            )
            .unwrap();
        assert_eq!(page.total, 12);
        assert_eq!(page.offset, 5);
        assert_eq!(page.items.len(), 4);
    }

    #[test]
    fn exclusions_only_change_statistics_and_manual_category_wins() {
        let fixture = Fixture::new("classification");
        fixture.insert_app(0, 60_000, "Editor", false);
        let id = fixture
            .1
            .connection()
            .unwrap()
            .query_row("SELECT id FROM activity_segments", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        fixture
            .1
            .set_default_category("application", "Editor.exe", "工作")
            .unwrap();
        fixture
            .1
            .set_activity_category("application", id, Some("学习"))
            .unwrap();
        let stats = fixture.query(0, 60_000, "hour");
        assert_eq!(stats.category_usage[0].app_name, "学习");
        fixture
            .1
            .set_statistics_excluded("application", "Editor.exe", true)
            .unwrap();
        assert_eq!(fixture.query(0, 60_000, "hour").effective_milliseconds, 0);
        assert_eq!(
            fixture
                .1
                .connection()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM activity_segments", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        fixture
            .1
            .set_statistics_excluded("application", "Editor.exe", false)
            .unwrap();
        assert_eq!(
            fixture.query(0, 60_000, "hour").effective_milliseconds,
            60_000
        );
    }

    #[test]
    fn month_buckets_follow_the_calendar_in_a_leap_year() {
        let fixture = Fixture::new("calendar");
        let start = Local
            .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let end = Local
            .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let stats = fixture.query(start, end, "month");
        assert_eq!(stats.trend.len(), 12);
        assert_eq!(stats.trend[1].label, "2024-02");
        assert_eq!(
            (stats.trend[2].started_at - stats.trend[1].started_at) / 86_400_000,
            29
        );
    }

    #[test]
    fn details_page_beyond_five_hundred_segments() {
        let fixture = Fixture::new("pagination");
        for index in 0..620 {
            fixture.insert_app(index * 2_000, index * 2_000 + 1_000, "Editor", false);
        }
        let details = fixture
            .1
            .query_activity_details(StatisticsQuery {
                start: 0,
                end: 2_000_000,
                bucket: "hour".into(),
                target_kind: Some("application".into()),
                target: Some("Editor.exe".into()),
                offset: Some(600),
                limit: Some(50),
                search: None,
                include_excluded: None,
                page_key: None,
            })
            .unwrap();
        assert_eq!(details.total, 620);
        assert_eq!(details.items.len(), 20);
    }

    #[test]
    fn compact_statistics_preserve_totals_and_target_filters() {
        let fixture = Fixture::new("compact-statistics");
        fixture.insert_app(0, 60_000, "Editor", false);
        fixture.insert_app(60_000, 75_000, "Editor", true);
        fixture.insert_app(75_000, 100_000, "Browser", false);
        let query = StatisticsQuery {
            start: 0,
            end: 100_000,
            bucket: "hour".into(),
            target_kind: Some("application".into()),
            target: Some("Editor.exe".into()),
            offset: None,
            limit: None,
            search: None,
            include_excluded: None,
            page_key: None,
        };
        let full = fixture.1.query_statistics(query.clone()).unwrap();
        let compact = fixture
            .1
            .query_statistics_with_options(query, false, false)
            .unwrap();
        assert!(!full.timeline.is_empty());
        assert!(!full.trend_series.is_empty());
        assert!(compact.timeline.is_empty());
        assert!(compact.trend_series.is_empty());
        assert_eq!(compact.effective_milliseconds, 60_000);
        assert_eq!(compact.tracked_milliseconds, 75_000);
        let mut full_json = serde_json::to_value(full).unwrap();
        full_json["timeline"] = serde_json::json!([]);
        full_json["trendSeries"] = serde_json::json!([]);
        assert_eq!(full_json, serde_json::to_value(compact).unwrap());
    }

    #[test]
    fn repeated_window_classification_cache_never_overrides_manual_corrections() {
        let fixture = Fixture::new("category-cache");
        for index in 0..3 {
            fixture.insert_app(index * 10_000, (index + 1) * 10_000, "Editor", false);
        }
        fixture
            .1
            .set_default_category("application", "Editor.exe", "工作")
            .unwrap();
        fixture
            .1
            .set_activity_category("application", 2, Some("学习"))
            .unwrap();
        let statistics = fixture.query(0, 30_000, "hour");
        assert_eq!(
            statistics
                .timeline
                .iter()
                .map(|row| row.category.as_str())
                .collect::<Vec<_>>(),
            ["工作", "学习", "工作"]
        );
        assert_eq!(statistics.category_usage[0].milliseconds, 20_000);
        assert_eq!(statistics.category_usage[1].milliseconds, 10_000);
    }

    /// Run explicitly when comparing the foreground statistics hot path. All rows
    /// live in Fixture's isolated temporary database, never the user's database.
    #[test]
    #[ignore = "isolated performance measurement"]
    fn statistics_performance_fixture() {
        let fixture = Fixture::new("performance");
        let mut connection = fixture.1.connection().unwrap();
        let transaction = connection.transaction().unwrap();
        {
            let mut insert = transaction.prepare("INSERT INTO activity_segments(started_at,ended_at,app_name,process_path,title,idle) VALUES(?1,?2,?3,?4,?5,0)").unwrap();
            for index in 0..12_000i64 {
                let app = index % 24;
                insert
                    .execute(params![
                        index * 60_000,
                        (index + 1) * 60_000,
                        format!("Application {app}"),
                        format!("C:\\Applications\\app{app}.exe"),
                        format!("Document {} — Application {app}", index % 80)
                    ])
                    .unwrap();
            }
            let mut insert = transaction.prepare("INSERT INTO classification_rules(name,target_kind,target_pattern,category,enabled,updated_at) VALUES(?1,'title',?2,?3,1,?4)").unwrap();
            for index in 0..80i64 {
                insert
                    .execute(params![
                        format!("Rule {index}"),
                        format!("Document {index} —"),
                        format!("Category {}", index % 5),
                        index
                    ])
                    .unwrap();
            }
        }
        transaction.commit().unwrap();
        drop(connection);
        let mut durations = Vec::new();
        let mut compact_durations = Vec::new();
        let mut compact_dashboard_durations = Vec::new();
        let mut full_bytes = 0;
        let mut compact_bytes = 0;
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let stats = fixture.query(0, 12_000 * 60_000, "day");
            durations.push(started.elapsed().as_micros());
            assert_eq!(stats.effective_milliseconds, 720_000_000);
            assert_eq!(stats.timeline.len(), 12_000);
            assert_eq!(stats.app_usage.len(), 24);
            assert_eq!(stats.category_usage.len(), 5);
            assert_eq!(
                stats
                    .trend
                    .iter()
                    .map(|point| point.milliseconds)
                    .sum::<i64>(),
                stats.effective_milliseconds
            );
            full_bytes = serde_json::to_vec(&stats).unwrap().len();
            let query = StatisticsQuery {
                start: 0,
                end: 12_000 * 60_000,
                bucket: "day".into(),
                target_kind: None,
                target: None,
                offset: None,
                limit: None,
                search: None,
                include_excluded: None,
                page_key: None,
            };
            let started = std::time::Instant::now();
            let compact = fixture
                .1
                .query_statistics_with_options(query, false, false)
                .unwrap();
            compact_durations.push(started.elapsed().as_micros());
            assert_eq!(compact.effective_milliseconds, stats.effective_milliseconds);
            assert_eq!(compact.records_count, stats.records_count);
            assert!(compact.timeline.is_empty());
            assert!(compact.trend_series.is_empty());
            compact_bytes = serde_json::to_vec(&compact).unwrap().len();
            let started = std::time::Instant::now();
            let dashboard = fixture.1.dashboard_with_statistics(false).unwrap();
            compact_dashboard_durations.push(started.elapsed().as_micros());
            assert!(dashboard.timeline.is_empty());
            assert!(dashboard.app_usage.is_empty());
            assert!(dashboard.current_activity.is_some());
        }
        eprintln!("statistics_performance_fixture rows=12000 rules=80 full_query_us={durations:?} compact_query_us={compact_durations:?} compact_dashboard_us={compact_dashboard_durations:?} full_json_bytes={full_bytes} compact_json_bytes={compact_bytes}");
    }
}
