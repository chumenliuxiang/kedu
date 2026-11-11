use crate::{category_models::*, AppState};
use tauri::State;
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn get_classification_workspace(
    state: State<'_, AppState>,
    query: ClassificationQuery,
) -> Result<ClassificationWorkspace, String> {
    let s = state.storage.clone();
    blocking(move || s.classification_workspace(query)).await
}
#[tauri::command]
pub async fn save_stat_category(
    state: State<'_, AppState>,
    input: NewStatCategory,
) -> Result<StatCategory, String> {
    let s = state.storage.clone();
    blocking(move || s.save_stat_category(input)).await
}
#[tauri::command]
pub async fn remove_stat_category(
    state: State<'_, AppState>,
    id: i64,
    destination_id: Option<i64>,
) -> Result<(), String> {
    let s = state.storage.clone();
    blocking(move || s.remove_stat_category(id, destination_id)).await
}
#[tauri::command]
pub async fn assign_stat_categories(
    state: State<'_, AppState>,
    keys: Vec<String>,
    category_id: Option<i64>,
) -> Result<(), String> {
    let s = state.storage.clone();
    blocking(move || s.assign_stat_categories(keys, category_id)).await
}
#[tauri::command]
pub async fn create_common_stat_categories(
    state: State<'_, AppState>,
) -> Result<Vec<StatCategory>, String> {
    let s = state.storage.clone();
    blocking(move || s.create_common_stat_categories()).await
}
#[tauri::command]
pub async fn save_classification_file(content: String, filename: String) -> Result<bool, String> {
    blocking(move || {
        if content.len() > 2_000_000 {
            return Err("导出内容过大".into());
        }
        let Some(path) = rfd::FileDialog::new()
            .set_title("导出分类清单")
            .set_file_name(
                std::path::Path::new(&filename)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("刻度分类清单.md"),
            )
            .add_filter("Markdown清单", &["md"])
            .save_file()
        else {
            return Ok(false);
        };
        std::fs::write(path, content).map_err(|e| e.to_string())?;
        Ok(true)
    })
    .await
}
