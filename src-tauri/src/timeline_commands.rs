use crate::{timeline_models::*, AppState};
use tauri::State;
#[tauri::command]
pub async fn get_timeline_workspace(
    state: State<'_, AppState>,
    start: i64,
    end: i64,
) -> Result<TimelineWorkspace, String> {
    let s = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || s.timeline_workspace(start, end))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn annotate_timeline(
    state: State<'_, AppState>,
    ranges: Vec<TimeRange>,
    category_id: Option<i64>,
    only_unclassified: bool,
) -> Result<AnnotationResult, String> {
    let s = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        s.annotate_timeline(ranges, category_id, only_unclassified)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn undo_timeline_annotation(
    state: State<'_, AppState>,
) -> Result<TimelineUndoResult, String> {
    let s = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || s.undo_timeline_annotation())
        .await
        .map_err(|e| e.to_string())?
}
