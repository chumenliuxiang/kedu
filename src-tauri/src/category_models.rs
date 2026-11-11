use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatCategory {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub application_count: usize,
    pub website_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationItem {
    pub key: String,
    pub kind: String,
    pub target: String,
    pub name: String,
    pub process_path: Option<String>,
    pub category_id: Option<i64>,
    pub category_name: String,
    pub last_seen_at: i64,
    pub milliseconds: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationQuery {
    pub kind: String,
    pub search: Option<String>,
    pub status: Option<String>,
    pub category_id: Option<i64>,
    pub recent_only: Option<bool>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationWorkspace {
    pub categories: Vec<StatCategory>,
    pub items: Vec<ClassificationItem>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub unclassified_count: usize,
    pub undo_available: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewStatCategory {
    pub id: Option<i64>,
    pub name: String,
    pub color: String,
}
