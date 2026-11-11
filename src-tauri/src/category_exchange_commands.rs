use crate::{category_exchange_models::*, AppState};
use tauri::State;
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn export_classification_package(
    state: State<'_, AppState>,
    keys: Vec<String>,
) -> Result<ClassificationExport, String> {
    let s = state.storage.clone();
    blocking(move || s.export_classification_package(keys)).await
}
#[tauri::command]
pub async fn preview_classification_import(
    state: State<'_, AppState>,
    content: String,
) -> Result<ImportPreview, String> {
    let s = state.storage.clone();
    blocking(move || s.preview_classification_import(content)).await
}
#[tauri::command]
pub async fn apply_classification_import(
    state: State<'_, AppState>,
    content: String,
    selected_keys: Vec<String>,
    token: String,
) -> Result<ImportResult, String> {
    let s = state.storage.clone();
    blocking(move || s.apply_classification_import(content, selected_keys, token)).await
}
#[tauri::command]
pub async fn undo_classification_import(state: State<'_, AppState>) -> Result<UndoResult, String> {
    let s = state.storage.clone();
    blocking(move || s.undo_classification_import()).await
}
