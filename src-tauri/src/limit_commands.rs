use crate::{
    domain::{LimitRule, NewLimitRule},
    AppState,
};
use tauri::State;

#[tauri::command]
pub async fn get_limit_reminders(
    state: State<'_, AppState>,
    rule_id: i64,
) -> Result<Vec<i64>, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || storage.get_limit_reminders(rule_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn save_limit_reminders(
    state: State<'_, AppState>,
    rule_id: i64,
    minutes: Vec<i64>,
) -> Result<Vec<i64>, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || storage.save_limit_reminders(rule_id, minutes))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn save_rule_with_reminders(
    state: State<'_, AppState>,
    id: Option<i64>,
    rule: NewLimitRule,
    minutes: Vec<i64>,
) -> Result<LimitRule, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        storage.save_rule_with_reminders(id, rule, minutes)
    })
    .await
    .map_err(|e| e.to_string())?
}
