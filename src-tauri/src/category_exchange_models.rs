use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportCategory {
    pub name: String,
    pub color: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportAssignment {
    pub key: String,
    pub category: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportDocument {
    pub version: u32,
    pub export_id: String,
    pub categories: Vec<ImportCategory>,
    pub assignments: Vec<ImportAssignment>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationExport {
    pub content: String,
    pub filename: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRow {
    pub key: String,
    pub name: String,
    pub kind: String,
    pub current_category: String,
    pub proposed_category: String,
    pub status: String,
    pub message: Option<String>,
    pub selectable: bool,
    pub selected: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub token: String,
    pub rows: Vec<ImportRow>,
    pub new_categories: Vec<ImportCategory>,
    pub warnings: Vec<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub applied: usize,
    pub created_categories: usize,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoResult {
    pub restored: usize,
    pub skipped: usize,
}
