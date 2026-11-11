use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub data_path: String,
    pub data_earliest_at: Option<i64>,
    pub data_latest_at: Option<i64>,
    pub last_sample_at: Option<i64>,
    pub last_write_at: Option<i64>,
    pub collector_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityIdentity {
    pub app_name: String,
    pub process_path: String,
    pub title: String,
    pub idle: bool,
    pub idle_seconds: u64,
    #[serde(default)]
    pub idle_exempt: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUsage {
    pub app_name: String,
    pub target: String,
    pub milliseconds: i64,
    pub seconds: i64,
    pub percent: f64,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetApplication {
    pub display_name: String,
    pub executable_name: String,
    pub path: String,
    pub source: String,
    pub last_seen_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetCatalog {
    pub applications: Vec<TargetApplication>,
    pub websites: Vec<String>,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItem {
    pub id: i64,
    pub app_name: String,
    pub title: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub seconds: i64,
    pub idle: bool,
    pub source_kind: String,
    pub target: String,
    pub domain: Option<String>,
    pub page_key: Option<String>,
    pub visit_id: Option<String>,
    pub source_ids: Option<Vec<i64>>,
    pub category: String,
    pub milliseconds: i64,
    pub color: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusState {
    pub id: i64,
    pub label: String,
    pub mode: String,
    pub started_at: i64,
    pub planned_end: i64,
    pub leave_budget_seconds: i64,
    pub leave_used_seconds: i64,
    pub leave_remaining_seconds: i64,
    pub active_leave_started_at: Option<i64>,
    pub pending_end_at: Option<i64>,
    pub status: String,
    pub task_id: Option<i64>,
    pub timer_kind: String,
    pub duration_seconds: i64,
    pub elapsed_milliseconds: i64,
    pub remaining_milliseconds: Option<i64>,
    pub leave_remaining_milliseconds: i64,
    pub observed_at: i64,
    pub break_ends_at: Option<i64>,
    pub rest_duration_seconds: i64,
    pub completed_at: Option<i64>,
    pub completion_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusTaskInput {
    pub id: Option<i64>,
    pub name: String,
    pub timer_kind: String,
    pub duration_minutes: i64,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusTask {
    pub id: i64,
    pub name: String,
    pub timer_kind: String,
    pub duration_minutes: i64,
    pub mode: String,
    pub today_completed: i64,
    pub today_milliseconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusWhitelistInput {
    pub id: Option<i64>,
    pub target_kind: String,
    pub target: String,
    pub display_name: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusWhitelistEntry {
    pub id: i64,
    pub target_kind: String,
    pub target: String,
    pub display_name: String,
    pub path: Option<String>,
    pub available_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusAppearance {
    pub style: String,
    pub size: String,
    pub auto_immerse: bool,
}

impl Default for FocusAppearance {
    fn default() -> Self {
        Self {
            style: "normal".into(),
            size: "normal".into(),
            auto_immerse: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FocusPreferences {
    pub window_guard_mode: String,
    pub pause_minutes: i64,
    pub rest_minutes: i64,
    pub notifications_enabled: bool,
    pub sound_enabled: bool,
    pub schedule_start_notifications_enabled: bool,
    pub schedule_start_sound_enabled: bool,
    pub schedule_end_notifications_enabled: bool,
    pub schedule_end_sound_enabled: bool,
}

impl Default for FocusPreferences {
    fn default() -> Self {
        Self {
            window_guard_mode: "reminder".into(),
            pause_minutes: 5,
            rest_minutes: 5,
            notifications_enabled: true,
            sound_enabled: true,
            schedule_start_notifications_enabled: true,
            schedule_start_sound_enabled: true,
            schedule_end_notifications_enabled: true,
            schedule_end_sound_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusWorkspace {
    pub tasks: Vec<FocusTask>,
    pub session: Option<FocusState>,
    pub whitelist: Vec<FocusWhitelistEntry>,
    pub appearance: FocusAppearance,
    pub preferences: FocusPreferences,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusNotification {
    pub session_id: i64,
    pub stage: String,
    pub label: String,
    pub occurred_at: i64,
    pub notifications_enabled: bool,
    pub sound_enabled: bool,
}

pub struct ScheduleNotification {
    pub session_id: Option<i64>,
    pub stage: &'static str,
    pub label: String,
    pub occurred_at: i64,
    pub notifications_enabled: bool,
    pub sound_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub generated_at: i64,
    pub day_started_at: i64,
    pub effective_seconds: i64,
    pub tracked_seconds: i64,
    pub effective_milliseconds: i64,
    pub tracked_milliseconds: i64,
    pub category_usage: Vec<AppUsage>,
    pub trend: Vec<TrendPoint>,
    pub current_activity: Option<ActivityIdentity>,
    pub app_usage: Vec<AppUsage>,
    pub site_usage: Vec<AppUsage>,
    pub timeline: Vec<TimelineItem>,
    pub focus: Option<FocusState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitRule {
    pub id: i64,
    pub name: String,
    pub target_kind: String,
    pub target_pattern: String,
    pub group_name: String,
    pub daily_limit_minutes: i64,
    pub enabled: bool,
    pub used_seconds: i64,
    pub remaining_seconds: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewLimitRule {
    pub name: String,
    pub target_kind: String,
    pub target_pattern: String,
    pub group_name: String,
    pub daily_limit_minutes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationRule {
    pub id: i64,
    pub name: String,
    pub target_kind: String,
    pub target_pattern: String,
    pub category: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewClassificationRule {
    pub name: String,
    pub target_kind: String,
    pub target_pattern: String,
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub idle_minutes: i64,
    pub count_media_while_idle: bool,
    pub meeting_apps: String,
    pub title_exclusions: String,
    pub day_start_hour: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestrictionCause {
    pub rule_id: i64,
    pub rule_name: String,
    pub kind: String,
    pub used_seconds: i64,
    pub limit_seconds: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestrictionNotice {
    pub app_name: String,
    pub title: String,
    pub causes: Vec<RestrictionCause>,
    pub subject_kind: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebActivity {
    pub domain: String,
    pub title: String,
    #[serde(default)]
    pub media_active: bool,
    #[serde(default)]
    pub idle: bool,
    #[serde(default)]
    pub idle_seconds: u64,
    #[serde(default)]
    pub idle_exempt: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebExceptionRequest {
    pub kind: String,
    pub rule_id: i64,
    pub minutes: i64,
    #[serde(default)]
    pub subject_kind: String,
    #[serde(default)]
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleImportItem {
    pub label: String,
    pub start: String,
    pub duration_minutes: i64,
    pub mode: String,
    #[serde(default)]
    pub task_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleImport {
    pub name: String,
    pub items: Vec<ScheduleImportItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleItem {
    pub id: i64,
    pub date: String,
    pub source_name: String,
    pub label: String,
    pub start: String,
    pub duration_minutes: i64,
    pub mode: String,
    pub enabled: bool,
    pub started: bool,
    pub task_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendPoint {
    pub started_at: i64,
    pub ended_at: i64,
    pub label: String,
    pub seconds: i64,
    pub milliseconds: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendSeries {
    pub kind: String,
    pub name: String,
    pub color: String,
    pub points: Vec<TrendPoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityStatistics {
    pub start: i64,
    pub end: i64,
    pub effective_seconds: i64,
    pub tracked_seconds: i64,
    pub effective_milliseconds: i64,
    pub tracked_milliseconds: i64,
    pub records_count: usize,
    pub app_usage: Vec<AppUsage>,
    pub site_usage: Vec<AppUsage>,
    pub category_usage: Vec<AppUsage>,
    pub trend: Vec<TrendPoint>,
    pub trend_series: Vec<TrendSeries>,
    pub timeline: Vec<TimelineItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsQuery {
    pub start: i64,
    pub end: i64,
    pub bucket: String,
    pub target_kind: Option<String>,
    pub target: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub search: Option<String>,
    pub include_excluded: Option<bool>,
    pub page_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsiteObservation {
    pub client_id: String,
    pub sequence: i64,
    pub observed_at: i64,
    pub focused: bool,
    pub window_id: Option<i64>,
    pub tab_id: Option<i64>,
    pub page_kind: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub page_key: String,
    #[serde(default)]
    pub media_active: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsiteRecordingStatus {
    pub bridge_online: bool,
    pub extension_online: bool,
    pub page_confirmed: bool,
    pub state: String,
    pub last_received_at: Option<i64>,
    pub last_written_at: Option<i64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsiteObservationResult {
    pub accepted: bool,
    pub recording: WebsiteRecordingStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsitePageUsage {
    pub site: String,
    pub page_key: String,
    pub title: String,
    pub domain: String,
    pub milliseconds: i64,
    pub seconds: i64,
    pub visits: usize,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsitePages {
    pub items: Vec<WebsitePageUsage>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityDetails {
    pub items: Vec<TimelineItem>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankingPage {
    pub items: Vec<AppUsage>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsExclusion {
    pub target_kind: String,
    pub target: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleChangeResult {
    pub pending: bool,
    pub apply_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionUsage {
    pub id: i64,
    pub kind: String,
    pub rule_name: String,
    pub subject: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub granted_seconds: i64,
    pub actual_seconds: i64,
    pub reason: String,
}
