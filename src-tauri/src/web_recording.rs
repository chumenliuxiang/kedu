//! Recording-only browser protocol. Reachability, reception and confirmed writing
//! are separate states; an old page is never reused after losing its foreground lease.
use super::*;
use crate::domain::{WebsiteObservation, WebsiteObservationResult, WebsiteRecordingStatus};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

const LEASE_MS: i64 = 8_000;
const ONLINE_MS: i64 = 40_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActivePage {
    owner: String,
    client: String,
    window_id: i64,
    tab_id: i64,
    native_window: isize,
    native_title: String,
    #[serde(default)]
    app_name: String,
    #[serde(default)]
    process_path: String,
    domain: String,
    title: String,
    page_key: String,
    visit_id: String,
    segment_id: i64,
    last_at: i64,
    lease_until: i64,
    media_active: bool,
    idle: bool,
    idle_exempt: bool,
}

fn owner() -> &'static str {
    static OWNER: OnceLock<String> = OnceLock::new();
    OWNER.get_or_init(|| {
        format!(
            "{}:{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        )
    })
}

pub(super) fn site_identity(domain: &str) -> (String, String) {
    let domain = normalize_domain(domain);
    let is_ip = domain
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .is_ok();
    let site = if is_ip {
        domain.clone()
    } else {
        psl::domain_str(&domain).unwrap_or(&domain).to_string()
    };
    let name = match site.as_str() {
        "bilibili.com" => "哔哩哔哩",
        "douyin.com" => "抖音",
        "youtube.com" | "youtu.be" => "YouTube",
        "zhihu.com" => "知乎",
        "xiaohongshu.com" => "小红书",
        _ => &site,
    }
    .to_string();
    (
        if site == "youtu.be" {
            "youtube.com".into()
        } else {
            site
        },
        name,
    )
}

/// Accept only a sanitized, same-host HTTP page identity. Reconstruct instead
/// of retaining a caller's query string, fragment, credentials or tracking data.
pub(super) fn clean_page_key(domain: &str, value: &str) -> Result<String, String> {
    let parsed = url::Url::parse(&format!("https://{value}")).map_err(|_| "页面标识无效")?;
    let host = normalize_domain(parsed.host_str().ok_or("页面域名无效")?);
    let source_host = normalize_domain(domain);
    let same_known_site = site_identity(&host).0 == site_identity(&source_host).0
        && matches!(
            site_identity(&host).0.as_str(),
            "bilibili.com" | "douyin.com" | "youtube.com"
        );
    if (host != source_host && !same_known_site)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || value.len() > 4096
        || parsed.path().len() > 2048
    {
        return Err("页面标识与域名不一致".into());
    }
    let path = parsed.path();
    let site = site_identity(&host).0;
    let identity = |key: &str| {
        parsed
            .query_pairs()
            .find(|(name, value)| {
                name == key
                    && !value.is_empty()
                    && value.len() <= 160
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            })
            .map(|(_, v)| v.into_owned())
    };
    if site == "douyin.com" {
        if let Some(id) = identity("modal_id") {
            return Ok(format!("douyin.com/video/{id}"));
        }
        if let Some(id) = path
            .strip_prefix("/video/")
            .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        {
            return Ok(format!("douyin.com/video/{id}"));
        }
    }
    if site == "youtube.com" && path == "/watch" {
        if let Some(id) = identity("v") {
            return Ok(format!("youtube.com/watch?v={id}"));
        }
    }
    if host == "youtu.be" {
        let id = path.trim_matches('/');
        if !id.is_empty()
            && id.len() <= 160
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Ok(format!("youtube.com/watch?v={id}"));
        }
    }
    let mut key = format!("{host}{path}");
    if site == "bilibili.com" && path.starts_with("/video/") {
        key = format!("bilibili.com{}", path.trim_end_matches('/'));
        if let Some(part) = identity("p")
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|part| *part > 1)
        {
            key.push_str(&format!("?p={part}"));
        }
    }
    Ok(key)
}

pub(super) fn migrate(tx: &rusqlite::Transaction<'_>) -> Result<(), String> {
    for (name, declaration) in [
        ("page_key", "TEXT NOT NULL DEFAULT ''"),
        ("visit_id", "TEXT NOT NULL DEFAULT ''"),
        ("browser_target", "TEXT NOT NULL DEFAULT 'msedge.exe'"),
    ] {
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('web_activity_segments') WHERE name=?1)", [name], |row| row.get(0)).map_err(|e| e.to_string())?;
        if !exists {
            tx.execute_batch(&format!(
                "ALTER TABLE web_activity_segments ADD COLUMN {name} {declaration}"
            ))
            .map_err(|e| e.to_string())?;
        }
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS web_recording_clients (
        client_id TEXT PRIMARY KEY, sequence INTEGER NOT NULL, observed_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS web_recording_state (
        id INTEGER PRIMARY KEY CHECK(id=1), active TEXT,
        last_received_at INTEGER, last_written_at INTEGER, state TEXT NOT NULL DEFAULT 'not_connected');
        INSERT OR IGNORE INTO web_recording_state(id) VALUES(1);
        CREATE INDEX IF NOT EXISTS idx_web_page ON web_activity_segments(page_key,started_at);").map_err(|e| e.to_string())
}

pub(super) fn validate(connection: &Connection) -> Result<(), String> {
    connection
        .prepare("SELECT page_key,visit_id,browser_target FROM web_activity_segments")
        .map_err(|e| e.to_string())?;
    connection
        .prepare("SELECT client_id,sequence,observed_at FROM web_recording_clients")
        .map_err(|e| e.to_string())?;
    let active: Option<String> = connection
        .query_row(
            "SELECT active FROM web_recording_state WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if let Some(value) = active {
        let _: ActivePage = serde_json::from_str(&value).map_err(|_| "备份的网站采集状态无效")?;
    }
    Ok(())
}

fn read_active(connection: &Connection) -> Result<Option<ActivePage>, String> {
    let value: Option<String> = connection
        .query_row(
            "SELECT active FROM web_recording_state WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| "网站采集状态无效".into()))
        .transpose()
}

fn save_active(
    connection: &Connection,
    active: Option<&ActivePage>,
    state: &str,
) -> Result<(), String> {
    let serialized = active
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| e.to_string())?;
    connection
        .execute(
            "UPDATE web_recording_state SET active=?1,state=?2 WHERE id=1",
            params![serialized, state],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn flush(connection: &Connection, active: &mut ActivePage, end: i64) -> Result<(), String> {
    if active.owner != owner() {
        return Ok(());
    }
    // An expired browser lease is not evidence of continued use. Never fill a
    // crash, suspend or extension-disconnected interval up to the timeout.
    let end = if end > active.lease_until {
        active.last_at
    } else {
        end.max(active.last_at)
    };
    if end > active.last_at {
        fill_native_gaps(connection, active, end)?;
        connection
            .execute(
                "UPDATE web_activity_segments SET ended_at=MAX(ended_at,?1) WHERE id=?2",
                params![end, active.segment_id],
            )
            .map_err(|e| e.to_string())?;
        connection
            .execute(
                "UPDATE web_recording_state SET last_written_at=?1 WHERE id=1",
                [end],
            )
            .map_err(|e| e.to_string())?;
        active.last_at = end;
    }
    Ok(())
}

/// A confirmed native window may be visited entirely between collector polls.
/// Add only its uncovered time; existing foreground rows always take precedence
/// and daily quota queries never see overlapping duplicate Edge durations.
fn fill_native_gaps(connection: &Connection, page: &ActivePage, end: i64) -> Result<(), String> {
    if page.process_path.is_empty() {
        return Ok(());
    }
    let mut statement = connection.prepare("SELECT started_at,ended_at FROM activity_segments WHERE started_at<?2 AND ended_at>?1 AND ended_at>started_at ORDER BY started_at,ended_at").map_err(|e|e.to_string())?;
    let spans = statement
        .query_map(params![page.last_at, end], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut cursor = page.last_at;
    let insert = |start: i64, stop: i64| -> Result<(), String> {
        if stop > start {
            connection.execute("INSERT INTO activity_segments(started_at,ended_at,app_name,process_path,title,idle,idle_exempt) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![start,stop,page.app_name,page.process_path,page.title,page.idle,page.idle_exempt]).map_err(|e|e.to_string())?;
        }
        Ok(())
    };
    for (start, stop) in spans {
        insert(cursor, start.min(end))?;
        cursor = cursor.max(stop).min(end);
        if cursor == end {
            break;
        }
    }
    insert(cursor, end)
}

fn edge(activity: &ActivityIdentity) -> bool {
    normalize_application_subject(&activity.process_path) == "msedge"
}

fn title_matches(title: &str, native: &str) -> bool {
    let title = title.trim();
    !title.is_empty()
        && (native == title
            || [" - ", " — ", " – ", " 和另外 ", " and "]
                .iter()
                .any(|suffix| native.starts_with(&format!("{title}{suffix}"))))
}

fn native_title_key(title: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    title.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

impl Storage {
    pub fn observe_website(
        &self,
        observation: WebsiteObservation,
    ) -> Result<WebsiteObservationResult, String> {
        self.observe_website_at(
            observation,
            Utc::now().timestamp_millis(),
            crate::platform::foreground_window_id(),
            crate::platform::foreground_activity(u64::MAX),
        )
    }

    fn observe_website_at(
        &self,
        observation: WebsiteObservation,
        now: i64,
        native_window: isize,
        activity: Option<ActivityIdentity>,
    ) -> Result<WebsiteObservationResult, String> {
        if observation.client_id.is_empty()
            || observation.client_id.len() > 100
            || observation.sequence < 1
            || now.abs_diff(observation.observed_at) > 15_000
            || observation.title.len() > 4000
            || observation.domain.len() > 253
            || !matches!(
                observation.page_kind.as_str(),
                "website" | "internal" | "local" | "unknown"
            )
        {
            return Err("网站观察格式或时间无效".into());
        }
        let settings = self.settings()?;
        let domain = normalize_domain(&observation.domain);
        let mut page_key = if observation.focused && observation.page_kind == "website" {
            clean_page_key(&domain, &observation.page_key)?
        } else {
            String::new()
        };
        let mut title = observation
            .title
            .trim()
            .chars()
            .take(500)
            .collect::<String>();
        if listed_identity_matches(&settings.title_exclusions, &domain)
            || activity.as_ref().is_some_and(|a| {
                listed_identity_matches(
                    &settings.title_exclusions,
                    &format!("{} {}", a.app_name, a.process_path),
                )
            })
        {
            title.clear();
            page_key = format!("{domain}/");
        }
        let at = observation.observed_at.min(now);
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let previous: Option<(i64, i64)> = tx
            .query_row(
                "SELECT sequence,observed_at FROM web_recording_clients WHERE client_id=?1",
                [&observation.client_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if previous.is_some_and(|(sequence, at)| {
            observation.sequence <= sequence || observation.observed_at < at
        }) {
            drop(tx);
            return Ok(WebsiteObservationResult {
                accepted: false,
                recording: self.website_recording_status_at(
                    now,
                    native_window,
                    activity.as_ref(),
                )?,
            });
        }
        tx.execute("INSERT INTO web_recording_clients(client_id,sequence,observed_at) VALUES(?1,?2,?3) ON CONFLICT(client_id) DO UPDATE SET sequence=excluded.sequence,observed_at=excluded.observed_at",params![observation.client_id,observation.sequence,observation.observed_at]).map_err(|e|e.to_string())?;
        tx.execute(
            "UPDATE web_recording_state SET last_received_at=?1 WHERE id=1",
            [now],
        )
        .map_err(|e| e.to_string())?;
        let mut previous_active = read_active(&tx)?.filter(|page| page.owner == owner());
        let confirmed = observation.focused
            && observation.page_kind == "website"
            && !domain.is_empty()
            && observation.window_id.is_some()
            && observation.tab_id.is_some()
            && native_window != 0
            && activity.as_ref().is_some_and(|activity| {
                edge(activity) && title_matches(&observation.title, &activity.title)
            });
        // An observation from another background profile must not clear a currently
        // confirmed profile. Its heartbeat still updates connection health.
        if previous_active.as_ref().is_some_and(|page| {
            at < page.last_at
                || (!confirmed && page.client != observation.client_id)
                || (!observation.focused
                    && observation.tab_id.is_some_and(|tab| {
                        tab != page.tab_id
                            || observation
                                .window_id
                                .is_some_and(|window| window != page.window_id)
                    }))
        }) {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(WebsiteObservationResult {
                accepted: true,
                recording: self.website_recording_status_at(
                    now,
                    native_window,
                    activity.as_ref(),
                )?,
            });
        }
        if let Some(page) = previous_active.as_mut() {
            flush(&tx, page, at)?;
        }
        if confirmed {
            let activity = activity.as_ref().expect("confirmed activity");
            let idle_exempt = settings.count_media_while_idle && observation.media_active;
            let idle =
                activity.idle_seconds >= settings.idle_minutes.max(1) as u64 * 60 && !idle_exempt;
            let same_visit = previous_active.as_ref().filter(|page| {
                page.client == observation.client_id
                    && page.window_id == observation.window_id.unwrap()
                    && page.tab_id == observation.tab_id.unwrap()
                    && page.page_key == page_key
                    && page.lease_until >= at
            });
            let visit_id = same_visit
                .map(|page| page.visit_id.clone())
                .unwrap_or_else(|| format!("{}:{}", observation.client_id, observation.sequence));
            let reusable = same_visit.filter(|page| {
                page.title == title
                    && page.domain == domain
                    && page.media_active == observation.media_active
                    && page.idle == idle
                    && page.idle_exempt == idle_exempt
            });
            let segment_id = if let Some(page) = reusable {
                page.segment_id
            } else {
                tx.execute("INSERT INTO web_activity_segments(started_at,ended_at,domain,title,media_active,idle,idle_exempt,page_key,visit_id,browser_target) VALUES(?1,?1,?2,?3,?4,?5,?6,?7,?8,'msedge.exe')",params![at,domain,title,observation.media_active,idle,idle_exempt,page_key,visit_id]).map_err(|e|e.to_string())?;
                tx.last_insert_rowid()
            };
            // A zero-length native anchor prevents the regular collector from
            // merging an unrelated application across this short Edge visit.
            if reusable.is_none() {
                tx.execute("INSERT INTO activity_segments(started_at,ended_at,app_name,process_path,title,idle,idle_exempt) VALUES(?1,?1,?2,?3,?4,?5,?6)",params![at,activity.app_name,activity.process_path,title,idle,idle_exempt]).map_err(|e|e.to_string())?;
            }
            let active = ActivePage {
                owner: owner().into(),
                client: observation.client_id,
                window_id: observation.window_id.unwrap(),
                tab_id: observation.tab_id.unwrap(),
                native_window,
                native_title: native_title_key(&activity.title),
                app_name: activity.app_name.clone(),
                process_path: activity.process_path.clone(),
                domain,
                title,
                page_key,
                visit_id,
                segment_id,
                last_at: at,
                lease_until: at + LEASE_MS,
                media_active: observation.media_active,
                idle,
                idle_exempt,
            };
            save_active(&tx, Some(&active), if idle { "idle" } else { "recording" })?;
        } else {
            save_active(
                &tx,
                None,
                if !observation.focused {
                    "inactive"
                } else if observation.page_kind != "website" {
                    "unsupported_page"
                } else {
                    "unconfirmed"
                },
            )?;
        }
        // Bound obsolete client leases while retaining normal profiles across restarts.
        tx.execute(
            "DELETE FROM web_recording_clients WHERE observed_at<?1",
            [now - 30 * 86_400_000],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(WebsiteObservationResult {
            accepted: true,
            recording: self.website_recording_status_at(now, native_window, activity.as_ref())?,
        })
    }

    pub fn tick_website_recording(&self) -> Result<(), String> {
        self.tick_website_recording_at(
            Utc::now().timestamp_millis(),
            crate::platform::foreground_window_id(),
            crate::platform::foreground_activity(u64::MAX).as_ref(),
        )
    }

    fn tick_website_recording_at(
        &self,
        now: i64,
        window: isize,
        activity: Option<&ActivityIdentity>,
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let Some(page) = read_active(&connection)? else {
            return Ok(());
        };
        if page.owner == owner()
            && now <= page.lease_until
            && window == page.native_window
            && activity.is_some_and(|value| {
                edge(value) && page.native_title == native_title_key(&value.title)
            })
        {
            return Ok(());
        }
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if let Some(mut current) = read_active(&tx)? {
            if current.segment_id == page.segment_id && current.last_at == page.last_at {
                flush(&tx, &mut current, now)?;
                save_active(
                    &tx,
                    None,
                    if now > current.lease_until {
                        "stale"
                    } else {
                        "inactive"
                    },
                )?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn website_recording_status(&self) -> Result<WebsiteRecordingStatus, String> {
        self.website_recording_status_at(
            Utc::now().timestamp_millis(),
            crate::platform::foreground_window_id(),
            crate::platform::foreground_activity(u64::MAX).as_ref(),
        )
    }

    pub fn confirmed_website_media(
        &self,
        activity: &ActivityIdentity,
    ) -> Result<Option<bool>, String> {
        let connection = self.connection()?;
        let received: Option<i64> = connection
            .query_row(
                "SELECT last_received_at FROM web_recording_state WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if received.is_none() {
            return Ok(None);
        }
        let now = Utc::now().timestamp_millis();
        let window = crate::platform::foreground_window_id();
        Ok(Some(read_active(&connection)?.is_some_and(|page| {
            page.owner == owner()
                && page.lease_until >= now
                && page.native_window == window
                && page.media_active
                && edge(activity)
                && page.native_title == native_title_key(&activity.title)
        })))
    }

    fn website_recording_status_at(
        &self,
        now: i64,
        window: isize,
        activity: Option<&ActivityIdentity>,
    ) -> Result<WebsiteRecordingStatus, String> {
        let connection = self.connection()?;
        let (received, written, stored): (Option<i64>, Option<i64>, String) = connection
            .query_row(
                "SELECT last_received_at,last_written_at,state FROM web_recording_state WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| e.to_string())?;
        let online = received.is_some_and(|at| now.saturating_sub(at) <= ONLINE_MS);
        let active = read_active(&connection)?;
        let confirmed = online
            && active.as_ref().is_some_and(|page| {
                page.owner == owner()
                    && now <= page.lease_until
                    && page.native_window == window
                    && activity
                        .is_some_and(|a| edge(a) && page.native_title == native_title_key(&a.title))
            });
        let state = if !online {
            "not_connected"
        } else if confirmed {
            if active.as_ref().is_some_and(|p| p.idle) {
                "idle"
            } else {
                "recording"
            }
        } else if stored == "recording" || stored == "idle" {
            "unconfirmed"
        } else {
            &stored
        };
        let message = match state {
            "not_connected" => "网站记录扩展未连接",
            "recording" => "正在记录当前网页",
            "idle" => "闲置中",
            "inactive" => "浏览器未在前台",
            "unsupported_page" => "当前页面不记录",
            "stale" => "网页采集已中断",
            _ => "等待确认当前网页",
        };
        Ok(WebsiteRecordingStatus {
            bridge_online: true,
            extension_online: online,
            page_confirmed: confirmed,
            state: state.into(),
            last_received_at: received,
            last_written_at: written,
            message: message.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::StatisticsQuery;

    struct Fixture {
        path: PathBuf,
        storage: Storage,
    }
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join("kedu-tests").join(format!(
                "website-{}-{}.sqlite",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap()
            ));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            Self {
                storage: Storage::new(path.clone()).unwrap(),
                path,
            }
        }
        fn query(&self) -> StatisticsQuery {
            StatisticsQuery {
                start: 100_000,
                end: 200_000,
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
        fn observe(
            &self,
            sequence: i64,
            at: i64,
            focused: bool,
            title: &str,
            page: &str,
        ) -> WebsiteObservationResult {
            self.storage
                .observe_website_at(
                    observation(sequence, at, focused, title, page),
                    at,
                    if focused { 7 } else { 9 },
                    if focused {
                        Some(edge_activity(title))
                    } else {
                        None
                    },
                )
                .unwrap()
        }
        fn total(&self) -> i64 {
            self.storage
                .query_statistics(self.query())
                .unwrap()
                .effective_milliseconds
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for path in [
                &self.path,
                &self.path.with_extension("sqlite-wal"),
                &self.path.with_extension("sqlite-shm"),
            ] {
                let _ = fs::remove_file(path);
            }
        }
    }
    fn edge_activity(title: &str) -> ActivityIdentity {
        ActivityIdentity {
            app_name: "Microsoft Edge".into(),
            process_path: "C:\\Edge\\msedge.exe".into(),
            title: format!("{title} - Microsoft Edge"),
            idle: false,
            idle_seconds: 0,
            idle_exempt: false,
        }
    }
    fn observation(
        sequence: i64,
        at: i64,
        focused: bool,
        title: &str,
        page: &str,
    ) -> WebsiteObservation {
        WebsiteObservation {
            client_id: "profile-a".into(),
            sequence,
            observed_at: at,
            focused,
            window_id: Some(1),
            tab_id: Some(2),
            page_kind: "website".into(),
            domain: "www.bilibili.com".into(),
            title: title.into(),
            page_key: page.into(),
            media_active: false,
        }
    }

    #[test]
    fn sub_poll_visit_survives_in_final_statistics_and_duplicate_delivery_is_idempotent() {
        let f = Fixture::new();
        f.observe(1, 100_000, true, "视频", "bilibili.com/video/BV123");
        f.observe(2, 102_500, false, "视频", "bilibili.com/video/BV123");
        assert!(
            !f.observe(2, 102_500, false, "视频", "bilibili.com/video/BV123")
                .accepted
        );
        let stats = f.storage.query_statistics(f.query()).unwrap();
        assert_eq!(stats.effective_milliseconds, 2500);
        assert_eq!(stats.site_usage[0].milliseconds, 2500);
        assert_eq!(stats.site_usage[0].target, "bilibili.com");
        assert_eq!(stats.site_usage[0].app_name, "哔哩哔哩");
        let reopened = Storage::new(f.path.clone()).unwrap();
        assert!(
            !reopened
                .observe_website_at(
                    observation(1, 100_000, true, "视频", "bilibili.com/video/BV123"),
                    103_000,
                    7,
                    Some(edge_activity("视频"))
                )
                .unwrap()
                .accepted
        );
        assert_eq!(f.total(), 2500);
    }

    #[test]
    fn confirmed_native_gaps_do_not_double_count_existing_collector_rows() {
        let f = Fixture::new();
        f.observe(1, 100_000, true, "视频", "bilibili.com/video/BV123");
        f.storage.connection().unwrap().execute("INSERT INTO activity_segments(started_at,ended_at,app_name,process_path,title,idle) VALUES(100500,101500,'Microsoft Edge','C:\\Edge\\msedge.exe','视频',0)",[]).unwrap();
        f.observe(2, 102_500, false, "视频", "bilibili.com/video/BV123");
        let raw: i64 = f
            .storage
            .connection()
            .unwrap()
            .query_row(
                "SELECT SUM(ended_at-started_at) FROM activity_segments",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(raw, 2500);
        assert_eq!(f.total(), 2500);
    }

    #[test]
    fn navigation_revisit_and_title_update_use_page_identity_with_visit_counts() {
        let f = Fixture::new();
        f.observe(
            1,
            100_000,
            true,
            "课程旧名",
            "bilibili.com/video/BV123?spm_id_from=tracking",
        );
        f.observe(2, 101_000, true, "课程新名", "bilibili.com/video/BV123");
        f.observe(3, 102_000, true, "另一个视频", "bilibili.com/video/BV456");
        f.observe(4, 103_000, true, "课程新名", "bilibili.com/video/BV123");
        f.observe(5, 104_000, false, "课程新名", "bilibili.com/video/BV123");
        let pages = f
            .storage
            .query_website_pages(f.query(), "bilibili.com", 0, 50, "")
            .unwrap();
        assert_eq!(pages.total, 2);
        assert_eq!(pages.items[0].page_key, "bilibili.com/video/BV123");
        assert_eq!(pages.items[0].milliseconds, 3000);
        assert_eq!(pages.items[0].visits, 2);
        assert_eq!(pages.items[0].title, "课程新名");
        let searched = f
            .storage
            .query_website_pages(f.query(), "bilibili.com", 0, 50, "课程旧名")
            .unwrap();
        assert_eq!(searched.total, 1);
        assert_eq!(searched.items[0].milliseconds, 3000);
        let mut query = f.query();
        query.target_kind = Some("website".into());
        query.target = Some("bilibili.com".into());
        query.page_key = Some(pages.items[0].page_key.clone());
        assert_eq!(
            f.storage
                .query_statistics(query.clone())
                .unwrap()
                .effective_milliseconds,
            3000
        );
        assert!(f
            .storage
            .query_activity_details(query)
            .unwrap()
            .items
            .iter()
            .all(|row| row.page_key.as_deref() == Some("bilibili.com/video/BV123")));
    }

    #[test]
    fn stale_and_background_clients_cannot_replace_confirmed_foreground() {
        let f = Fixture::new();
        f.observe(1, 100_000, true, "课程", "bilibili.com/video/BV123");
        f.observe(2, 105_000, true, "课程", "bilibili.com/video/BV123");
        let mut other = observation(1, 104_000, true, "另一窗口", "bilibili.com/video/BV456");
        other.client_id = "profile-b".into();
        f.storage
            .observe_website_at(other, 105_100, 7, Some(edge_activity("课程")))
            .unwrap();
        let mut other = observation(2, 105_500, false, "", "bilibili.com/");
        other.client_id = "profile-b".into();
        f.storage
            .observe_website_at(other, 105_500, 7, Some(edge_activity("课程")))
            .unwrap();
        f.observe(3, 106_000, false, "课程", "bilibili.com/video/BV123");
        assert_eq!(f.total(), 6000);
        let pages = f
            .storage
            .query_website_pages(f.query(), "bilibili.com", 0, 50, "")
            .unwrap();
        assert_eq!(pages.total, 1);
        assert_eq!(pages.items[0].visits, 1);
    }

    #[test]
    fn expired_or_previous_process_leases_never_fabricate_offline_usage() {
        let f = Fixture::new();
        f.observe(1, 100_000, true, "课程", "bilibili.com/video/BV123");
        f.observe(2, 105_000, true, "课程", "bilibili.com/video/BV123");
        f.storage
            .tick_website_recording_at(150_000, 7, Some(&edge_activity("课程")))
            .unwrap();
        assert_eq!(f.total(), 5000);
        f.observe(3, 160_000, true, "课程", "bilibili.com/video/BV123");
        let connection = f.storage.connection().unwrap();
        let mut active = read_active(&connection).unwrap().unwrap();
        active.owner = "previous-process".into();
        save_active(&connection, Some(&active), "recording").unwrap();
        drop(connection);
        f.observe(4, 164_000, true, "课程", "bilibili.com/video/BV123");
        f.observe(5, 166_000, false, "课程", "bilibili.com/video/BV123");
        assert_eq!(f.total(), 7000);
        let status = f
            .storage
            .website_recording_status_at(250_000, 7, Some(&edge_activity("课程")))
            .unwrap();
        assert!(!status.extension_online);
        assert!(!status.page_confirmed);
    }

    #[test]
    fn foreground_event_closes_short_visit_and_idle_media_is_separate() {
        let f = Fixture::new();
        f.observe(1, 100_000, true, "课程", "bilibili.com/video/BV123");
        f.storage
            .tick_website_recording_at(101_000, 9, None)
            .unwrap();
        assert_eq!(f.total(), 1000);
        let mut idle = edge_activity("课程");
        idle.idle_seconds = 1000;
        f.storage
            .observe_website_at(
                observation(2, 102_000, true, "课程", "bilibili.com/video/BV123"),
                102_000,
                7,
                Some(idle.clone()),
            )
            .unwrap();
        f.observe(3, 103_000, false, "课程", "bilibili.com/video/BV123");
        assert_eq!(f.total(), 1000);
        let mut media = observation(4, 104_000, true, "课程", "bilibili.com/video/BV123");
        media.media_active = true;
        f.storage
            .observe_website_at(media, 104_000, 7, Some(idle))
            .unwrap();
        f.observe(5, 105_000, false, "课程", "bilibili.com/video/BV123");
        assert_eq!(f.total(), 2000);
    }

    #[test]
    fn parent_site_classification_and_exclusion_preserve_raw_history_and_pages() {
        let f = Fixture::new();
        f.observe(1, 100_000, true, "课程", "bilibili.com/video/BV123");
        f.observe(2, 102_000, false, "课程", "bilibili.com/video/BV123");
        f.storage
            .set_default_category("website", "bilibili.com", "学习")
            .unwrap();
        assert_eq!(
            f.storage
                .query_statistics(f.query())
                .unwrap()
                .category_usage[0]
                .app_name,
            "学习"
        );
        f.storage
            .set_statistics_excluded("website", "bilibili.com", true)
            .unwrap();
        assert_eq!(f.total(), 0);
        assert_eq!(
            f.storage
                .query_website_pages(f.query(), "bilibili.com", 0, 50, "")
                .unwrap()
                .total,
            0
        );
        f.storage
            .set_statistics_excluded("website", "bilibili.com", false)
            .unwrap();
        assert_eq!(f.total(), 2000);
    }

    #[test]
    fn page_keys_strip_tracking_but_preserve_supported_video_identity() {
        for part in ["1", "01"] {
            assert_eq!(
                clean_page_key(
                    "www.bilibili.com",
                    &format!("bilibili.com/video/BV123?p={part}")
                )
                .unwrap(),
                "bilibili.com/video/BV123"
            );
        }
        assert_eq!(
            clean_page_key("www.bilibili.com", "bilibili.com/video/BV123?p=02").unwrap(),
            "bilibili.com/video/BV123?p=2"
        );
        assert_eq!(
            clean_page_key(
                "www.bilibili.com",
                "bilibili.com/video/BV123/?p=2&spm=secret#x"
            )
            .unwrap(),
            "bilibili.com/video/BV123?p=2"
        );
        assert_eq!(
            clean_page_key(
                "www.douyin.com",
                "douyin.com/?modal_id=123&utm_source=secret"
            )
            .unwrap(),
            "douyin.com/video/123"
        );
        assert_eq!(
            clean_page_key("youtu.be", "youtube.com/watch?v=abc_123&si=secret").unwrap(),
            "youtube.com/watch?v=abc_123"
        );
        assert_eq!(
            clean_page_key("example.com", "example.com/course?token=secret#fragment").unwrap(),
            "example.com/course"
        );
        assert!(clean_page_key("example.com", "other.com/course").is_err());
        assert!(clean_page_key("bilibili.com", "bilibili.com.evil.com/video/BV123").is_err());
        assert_eq!(site_identity("a.b.example.co.uk").0, "example.co.uk");
        assert_eq!(site_identity("127.0.0.1").0, "127.0.0.1");
        assert_eq!(site_identity("192.168.1.7").0, "192.168.1.7");
        assert_eq!(site_identity("[::1]").0, "[::1]");
    }

    #[test]
    fn recording_backup_round_trip_and_invalid_state_never_overwrite_live_data() {
        let f = Fixture::new();
        f.observe(1, 100_000, true, "课程", "bilibili.com/video/BV123");
        f.observe(2, 102_000, false, "课程", "bilibili.com/video/BV123");
        let backup = f.path.with_extension("backup.sqlite");
        f.storage.backup_to(backup.clone()).unwrap();
        f.observe(3, 103_000, true, "课程", "bilibili.com/video/BV456");
        f.observe(4, 104_000, false, "课程", "bilibili.com/video/BV456");
        assert_eq!(f.total(), 3000);
        f.storage.restore_from(backup.clone()).unwrap();
        assert_eq!(f.total(), 2000);
        let page = f
            .storage
            .query_website_pages(f.query(), "bilibili.com", 0, 50, "")
            .unwrap();
        assert_eq!(page.items[0].page_key, "bilibili.com/video/BV123");
        let connection = Connection::open(&backup).unwrap();
        connection
            .execute(
                "UPDATE web_recording_state SET active='invalid json' WHERE id=1",
                [],
            )
            .unwrap();
        drop(connection);
        assert!(f.storage.restore_from(backup.clone()).is_err());
        assert_eq!(f.total(), 2000);
        fs::remove_file(backup).unwrap();
    }

    #[test]
    fn late_hidden_tab_does_not_clear_new_tab_but_window_focus_loss_does() {
        let f = Fixture::new();
        f.observe(1, 100_000, true, "旧标签", "bilibili.com/video/BV123");
        let mut next = observation(2, 101_000, true, "新标签", "bilibili.com/video/BV456");
        next.tab_id = Some(3);
        f.storage
            .observe_website_at(next, 101_000, 7, Some(edge_activity("新标签")))
            .unwrap();
        let hidden = observation(3, 101_500, false, "旧标签", "bilibili.com/video/BV123");
        let result = f
            .storage
            .observe_website_at(hidden, 101_500, 7, Some(edge_activity("新标签")))
            .unwrap();
        assert!(result.recording.page_confirmed);
        let mut focus_lost = observation(4, 103_000, false, "", "");
        focus_lost.tab_id = None;
        focus_lost.window_id = None;
        let result = f
            .storage
            .observe_website_at(focus_lost, 103_000, 9, None)
            .unwrap();
        assert!(!result.recording.page_confirmed);
        assert_eq!(result.recording.state, "inactive");
        let pages = f
            .storage
            .query_website_pages(f.query(), "bilibili.com", 0, 50, "")
            .unwrap();
        assert_eq!(pages.total, 2);
        assert_eq!(pages.items[0].page_key, "bilibili.com/video/BV456");
        assert_eq!(pages.items[0].milliseconds, 2000);
        assert_eq!(f.total(), 3000);
    }

    #[test]
    fn merged_visit_correction_updates_all_sources_and_invalid_id_rolls_back() {
        let f = Fixture::new();
        f.observe(1, 100_000, true, "课程旧名", "bilibili.com/video/BV123");
        f.observe(2, 101_000, true, "课程新名", "bilibili.com/video/BV123");
        f.observe(3, 102_000, false, "课程新名", "bilibili.com/video/BV123");
        let mut query = f.query();
        query.target_kind = Some("website".into());
        query.target = Some("bilibili.com".into());
        query.page_key = Some("bilibili.com/video/BV123".into());
        let details = f.storage.query_activity_details(query.clone()).unwrap();
        assert_eq!(details.total, 1);
        let row = &details.items[0];
        assert_eq!(row.milliseconds, 2000);
        assert_eq!(row.title, "课程新名");
        let ids = row.source_ids.as_ref().unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&row.id));
        f.storage
            .set_activity_categories("website", ids, Some("学习"))
            .unwrap();
        let after = f.storage.query_activity_details(query.clone()).unwrap();
        assert_eq!(after.total, 1);
        assert_eq!(after.items[0].category, "学习");
        let mut invalid = ids.clone();
        invalid.push(i64::MAX);
        assert!(f
            .storage
            .set_activity_categories("website", &invalid, Some("娱乐"))
            .is_err());
        let unchanged = f.storage.query_activity_details(query.clone()).unwrap();
        assert_eq!(unchanged.total, 1);
        assert_eq!(unchanged.items[0].category, "学习");
        f.storage
            .set_activity_categories("website", ids, None)
            .unwrap();
        let restored = f.storage.query_activity_details(query).unwrap();
        assert_eq!(restored.total, 1);
        assert_eq!(restored.items[0].category, "未分类");
    }
}
