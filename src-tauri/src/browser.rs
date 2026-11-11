//! The browser lease is deliberately separate from extension connectivity.
//! A healthy extension does not prove that its last reported page is still foreground.
use std::sync::{Mutex, OnceLock};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{
    domain::{ActivityIdentity, FocusState, RestrictionNotice, WebActivity},
    platform,
    storage::Storage,
};

const PAGE_LEASE_MS: i64 = 8_000;
const CONNECTION_LEASE_MS: i64 = 40_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserObservation {
    pub client_id: String,
    pub observation_id: String,
    pub observed_at: i64,
    pub focused: bool,
    #[serde(default)]
    pub tab_id: Option<i64>,
    #[serde(default)]
    pub window_id: Option<i64>,
    #[serde(default)]
    pub page_kind: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub media_active: bool,
    #[serde(default)]
    pub overlay_available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProtectionStatus {
    pub extension_online: bool,
    pub page_confirmed: bool,
    pub state: String,
    pub domain: Option<String>,
    pub message: String,
    pub last_seen_at: Option<i64>,
}

#[derive(Default)]
struct BrowserRuntime {
    last_seen_at: Option<i64>,
    page: Option<ConfirmedPage>,
    latest_observation: Option<(String, i64, String)>,
}

struct ConfirmedPage {
    observation: BrowserObservation,
    received_at: i64,
    native_window: isize,
    overlay_rendered: bool,
}

fn runtime() -> &'static Mutex<BrowserRuntime> {
    static RUNTIME: OnceLock<Mutex<BrowserRuntime>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(BrowserRuntime::default()))
}

pub fn native_window() -> isize {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
    }
    #[cfg(not(windows))]
    {
        0
    }
}

fn is_edge(activity: &ActivityIdentity) -> bool {
    activity
        .process_path
        .rsplit(['\\', '/'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("msedge.exe"))
}

fn title_matches(page_title: &str, native_title: &str) -> bool {
    let page_title = page_title.trim();
    !page_title.is_empty()
        && (native_title == page_title
            || native_title.starts_with(&format!("{page_title} - "))
            || native_title.starts_with(&format!("{page_title} — "))
            || native_title.starts_with(&format!("{page_title} – ")))
}

fn page_matches(
    page: &ConfirmedPage,
    activity: Option<&ActivityIdentity>,
    window: isize,
    now: i64,
) -> bool {
    now.saturating_sub(page.received_at) <= PAGE_LEASE_MS
        && page.native_window != 0
        && page.native_window == window
        && activity.is_some_and(|activity| {
            is_edge(activity) && title_matches(&page.observation.title, &activity.title)
        })
}

pub fn heartbeat() {
    if let Ok(mut runtime) = runtime().lock() {
        runtime.last_seen_at = Some(Utc::now().timestamp_millis());
    }
}

/// Only an observation corroborated by the actual Windows foreground can create a lease.
/// Unknown/internal pages never borrow a previous HTTP page's identity.
pub fn observe(observation: BrowserObservation) -> bool {
    let now = Utc::now().timestamp_millis();
    if observation.client_id.len() > 100
        || observation.observation_id.len() > 100
        || observation.title.len() > 4_000
        || observation.domain.len() > 253
        || now.abs_diff(observation.observed_at) > 15_000
    {
        return false;
    }
    let window = native_window();
    let activity = platform::foreground_activity(u64::MAX);
    let Ok(mut runtime) = runtime().lock() else {
        return false;
    };
    runtime.last_seen_at = Some(now);
    if runtime
        .latest_observation
        .as_ref()
        .is_some_and(|(client, at, _)| {
            *at > observation.observed_at && client == &observation.client_id
        })
    {
        return false;
    }
    runtime.latest_observation = Some((
        observation.client_id.clone(),
        observation.observed_at,
        observation.observation_id.clone(),
    ));
    if !observation.focused {
        if runtime
            .page
            .as_ref()
            .is_some_and(|page| page.observation.client_id == observation.client_id)
        {
            runtime.page = None;
        }
        return false;
    }
    let confirmed = observation.tab_id.is_some()
        && observation.window_id.is_some()
        && window != 0
        && activity.as_ref().is_some_and(|activity| {
            is_edge(activity) && title_matches(&observation.title, &activity.title)
        })
        && matches!(
            observation.page_kind.as_str(),
            "website" | "internal" | "local"
        )
        && (observation.page_kind != "website" || valid_domain(&observation.domain));
    let overlay_rendered = runtime.page.as_ref().is_some_and(|page| {
        page.overlay_rendered
            && observation.overlay_available
            && page.native_window == window
            && page.observation.client_id == observation.client_id
            && page.observation.tab_id == observation.tab_id
            && page.observation.window_id == observation.window_id
            && page.observation.domain == observation.domain
            && page.observation.title == observation.title
            && page.observation.page_kind == observation.page_kind
    });
    runtime.page = if confirmed {
        Some(ConfirmedPage {
            observation,
            received_at: now,
            native_window: window,
            overlay_rendered,
        })
    } else {
        None
    };
    confirmed
}

fn valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-:[]".contains(&byte))
}

pub fn observation_current(client: &str, observation: &str) -> bool {
    runtime().lock().is_ok_and(|runtime| {
        runtime
            .latest_observation
            .as_ref()
            .is_some_and(|(latest_client, _, latest_id)| {
                latest_client == client && latest_id == observation
            })
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserPolicy {
    connected: bool,
    pub page_confirmed: bool,
    observation_id: String,
    blocked: bool,
    notice: Option<RestrictionNotice>,
    focus: Option<FocusState>,
    protection: BrowserProtectionStatus,
}

/// No caller-supplied hostname is evaluated until it is corroborated by the OS.
pub fn policy(
    storage: &Storage,
    client: &str,
    observation: &str,
    record: bool,
) -> Result<BrowserPolicy, String> {
    let now = Utc::now().timestamp_millis();
    let foreground = platform::foreground_activity(u64::MAX);
    let window = native_window();
    let confirmed = {
        let runtime = runtime().lock().map_err(|_| "网站保护状态暂不可用")?;
        runtime
            .page
            .as_ref()
            .filter(|page| {
                page.observation.client_id == client
                    && page.observation.observation_id == observation
                    && page_matches(page, foreground.as_ref(), window, now)
            })
            .map(|page| page.observation.clone())
    };
    if let Some(page) = confirmed
        .as_ref()
        .filter(|page| page.page_kind == "website")
    {
        let settings = storage.settings()?;
        let idle_seconds = platform::idle_seconds();
        let idle_exempt = settings.count_media_while_idle && page.media_active;
        let activity = WebActivity {
            domain: page.domain.clone(),
            title: page.title.clone(),
            media_active: page.media_active,
            idle: idle_seconds >= settings.idle_minutes.max(1) as u64 * 60 && !idle_exempt,
            idle_seconds,
            idle_exempt,
        };
        if record {
            storage.record_web_activity(&activity)?;
        }
    }
    Ok(BrowserPolicy {
        connected: true,
        page_confirmed: confirmed.is_some(),
        observation_id: observation.into(),
        blocked: false,
        notice: None,
        focus: None,
        protection: protection_status(),
    })
}

pub fn acknowledge_overlay(client_id: &str, observation_id: &str, rendered: bool) {
    if let Ok(mut runtime) = runtime().lock() {
        if let Some(page) = runtime.page.as_mut().filter(|page| {
            page.observation.client_id == client_id
                && page.observation.observation_id == observation_id
        }) {
            page.overlay_rendered = rendered;
        }
    }
}

pub fn protection_status() -> BrowserProtectionStatus {
    let now = Utc::now().timestamp_millis();
    let activity = platform::foreground_activity(u64::MAX);
    let window = native_window();
    let Ok(runtime) = runtime().lock() else {
        return BrowserProtectionStatus {
            extension_online: false,
            page_confirmed: false,
            state: "unavailable".into(),
            domain: None,
            message: "网站保护暂不可用".into(),
            last_seen_at: None,
        };
    };
    let online = runtime
        .last_seen_at
        .is_some_and(|last| now.saturating_sub(last) <= CONNECTION_LEASE_MS);
    let confirmed = runtime
        .page
        .as_ref()
        .filter(|page| page_matches(page, activity.as_ref(), window, now));
    let (state, message) = if !online {
        ("offline", "Edge 扩展未连接，网站保护已暂停")
    } else if !activity.as_ref().is_some_and(is_edge) {
        ("inactive", "Edge 已连接")
    } else if let Some(page) = confirmed {
        match page.observation.page_kind.as_str() {
            "internal" | "local" => ("allowed_page", "当前为浏览器内置页或本地文件"),
            _ => ("protected", "网站保护正常"),
        }
    } else {
        ("unconfirmed", "无法确认当前网站，网站保护已暂停")
    };
    BrowserProtectionStatus {
        extension_online: online,
        page_confirmed: confirmed.is_some(),
        state: state.into(),
        domain: confirmed
            .filter(|page| page.observation.page_kind == "website")
            .map(|page| page.observation.domain.clone()),
        message: message.into(),
        last_seen_at: runtime.last_seen_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> ConfirmedPage {
        ConfirmedPage {
            observation: BrowserObservation {
                client_id: "test".into(),
                observation_id: "test-1".into(),
                observed_at: 1000,
                focused: true,
                tab_id: Some(1),
                window_id: Some(10),
                page_kind: "website".into(),
                domain: "learn.test".into(),
                title: "Lesson".into(),
                media_active: false,
                overlay_available: true,
            },
            received_at: 1000,
            native_window: 100,
            overlay_rendered: false,
        }
    }

    fn activity() -> ActivityIdentity {
        ActivityIdentity {
            app_name: "Edge".into(),
            process_path: "C:\\Edge\\msedge.exe".into(),
            title: "Lesson - Profile - Microsoft Edge".into(),
            idle: false,
            idle_seconds: 0,
            idle_exempt: false,
        }
    }

    #[test]
    fn foreground_page_requires_matching_window_title_and_fresh_lease() {
        let page = page();
        let mut activity = activity();
        assert!(page_matches(&page, Some(&activity), 100, 2000));
        assert!(!page_matches(&page, Some(&activity), 200, 2000));
        assert!(!page_matches(&page, Some(&activity), 100, 9001));
        activity.title = "Other tab - Microsoft Edge".into();
        assert!(!page_matches(&page, Some(&activity), 100, 2000));
        activity.title = "Lesson - Microsoft Edge".into();
        activity.process_path = "C:\\Other\\other.exe".into();
        assert!(!page_matches(&page, Some(&activity), 100, 2000));
    }

    #[test]
    fn empty_or_prefix_only_titles_never_confirm_the_wrong_page() {
        assert!(!title_matches("", "Microsoft Edge"));
        assert!(!title_matches("Lesson", "Lesson two - Microsoft Edge"));
        assert!(title_matches("Lesson", "Lesson — Microsoft Edge"));
    }

    #[test]
    fn page_identity_never_accepts_an_unrelated_or_expired_window() {
        let page = page();
        let activity = activity();
        assert!(!page_matches(&page, None, 100, 2000));
        assert!(!page_matches(&page, Some(&activity), 0, 2000));
        // Even an identical page title in another browser window cannot reuse a lease.
        assert!(!page_matches(&page, Some(&activity), 101, 2000));
        assert!(!page_matches(&page, Some(&activity), 100, 10_000));
    }

    #[test]
    fn observation_domain_is_not_a_url_or_script() {
        assert!(valid_domain("learn.example.com"));
        assert!(valid_domain("127.0.0.1"));
        assert!(valid_domain("[::1]"));
        assert!(!valid_domain(""));
        assert!(!valid_domain("https://learn.example.com/private"));
        assert!(!valid_domain("example.com?a=secret"));
        assert!(!valid_domain("<script>"));
    }
}
