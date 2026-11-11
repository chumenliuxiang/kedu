use crate::{category_models::StatCategory, domain::TimelineItem};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeRange {
    pub start: i64,
    pub end: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineWorkspace {
    pub items: Vec<TimelineItem>,
    pub categories: Vec<StatCategory>,
    pub undo_available: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationResult {
    pub changed_milliseconds: i64,
    pub undo_available: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineUndoResult {
    pub restored_milliseconds: i64,
}

pub(crate) struct TimelineCandidate {
    pub start: i64,
    pub end: i64,
    pub category: String,
    pub manual: bool,
    pub annotation_id: Option<i64>,
}
