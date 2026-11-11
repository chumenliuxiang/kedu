use tauri::State;

use crate::{
    storage::focus_statistics::{
        FocusRecordsPage, FocusRecordsQuery, FocusStatistics, FocusStatisticsQuery,
    },
    AppState,
};

#[tauri::command]
pub async fn query_focus_statistics(
    state: State<'_, AppState>,
    query: FocusStatisticsQuery,
) -> Result<FocusStatistics, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || storage.query_focus_statistics(query))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn list_focus_records(
    state: State<'_, AppState>,
    query: FocusRecordsQuery,
) -> Result<FocusRecordsPage, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || storage.list_focus_records(query))
        .await
        .map_err(|error| error.to_string())?
}
