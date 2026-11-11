//! Data lives outside AppData because MSIX callers can redirect AppData per package.
//! Never infer that an empty redirected directory means a user has no old data.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{backup::Backup, params, types::Value, Connection, OpenFlags, OptionalExtension};
use serde::Serialize;

use crate::storage::Storage;

const DATABASE_NAME: &str = "kedu.sqlite";
const HISTORY_TABLES: [&str; 2] = ["activity_segments", "web_activity_segments"];

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInventory {
    pub source: String,
    pub requested_path: String,
    pub resolved_path: Option<String>,
    pub exists: bool,
    pub tables: BTreeMap<String, i64>,
    pub earliest_at: Option<i64>,
    pub latest_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataDiagnostics {
    pub package_identity: Option<String>,
    pub destination: String,
    pub safe_to_migrate: bool,
    pub legacy_autostart_enabled: bool,
    pub databases: Vec<DatabaseInventory>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MigrationManifest {
    format_version: u32,
    created_at: i64,
    destination: String,
    primary_source: Option<String>,
    sources: Vec<DatabaseInventory>,
    recovery_directory: String,
    added_segments: BTreeMap<String, usize>,
    preserved_configurations: Vec<String>,
    preserved_autostart: Option<bool>,
}

#[cfg(windows)]
fn legacy_autostart_enabled() -> Result<bool, String> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let run = match current_user.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run") {
        Ok(run) => run,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("无法核实旧版开机启动状态：{error}")),
    };
    for value in run.enum_values() {
        let (name, _) = value.map_err(|error| format!("无法核实旧版开机启动状态：{error}"))?;
        let name = name.trim().to_lowercase();
        if matches!(
            name.as_str(),
            "kedu" | "刻度" | "刻度 kedu" | "io.local.kedu"
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(not(windows))]
fn legacy_autostart_enabled() -> Result<bool, String> {
    Ok(false)
}

fn text_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn identity_path(path: &Path) -> String {
    text_path(path)
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

#[cfg(windows)]
pub fn package_identity() -> Result<Option<String>, String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentPackageFullName(length: *mut u32, name: *mut u16) -> u32;
    }
    let mut length = 0;
    let result = unsafe { GetCurrentPackageFullName(&mut length, std::ptr::null_mut()) };
    // APPMODEL_ERROR_NO_PACKAGE is the only result proving an unpackaged process.
    if result == 15700 {
        return Ok(None);
    }
    if result != 122 || length == 0 {
        return Err(format!("无法核实 Windows 启动环境（错误 {result}）"));
    }
    let mut name = vec![0u16; length as usize];
    let result = unsafe { GetCurrentPackageFullName(&mut length, name.as_mut_ptr()) };
    if result != 0 {
        return Err(format!("无法读取 Windows 包身份（错误 {result}）"));
    }
    Ok(Some(String::from_utf16_lossy(
        &name[..length.saturating_sub(1) as usize],
    )))
}

#[cfg(not(windows))]
pub fn package_identity() -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(windows)]
pub fn resolved_file_path(path: &Path) -> Result<PathBuf, String> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFinalPathNameByHandleW(
            file: *mut std::ffi::c_void,
            path: *mut u16,
            length: u32,
            flags: u32,
        ) -> u32;
    }
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let length =
        unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), std::ptr::null_mut(), 0, 0) };
    if length == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let written = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            0,
        )
    };
    if written == 0 || written as usize >= buffer.len() {
        return Err("无法核实数据库实际位置".into());
    }
    Ok(PathBuf::from(String::from_utf16_lossy(
        &buffer[..written as usize],
    )))
}

#[cfg(not(windows))]
pub fn resolved_file_path(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|error| error.to_string())
}

fn profile_dir() -> Result<PathBuf, String> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| "无法获取当前用户目录，未创建数据库".into())
}

/// Tests must opt into an allowlisted disposable directory and an explicit sentinel.
pub fn test_data_dir() -> Result<Option<PathBuf>, String> {
    let Some(directory) = std::env::var_os("KEDU_TEST_DATA_DIR") else {
        return Ok(None);
    };
    let directory = PathBuf::from(directory)
        .canonicalize()
        .map_err(|error| format!("测试目录不存在：{error}"))?;
    let roots = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/kedu-tests"),
        std::env::temp_dir().join("kedu-tests"),
    ];
    let allowed = roots.iter().any(|root| {
        root.canonicalize()
            .map(|root| directory.starts_with(root))
            .unwrap_or(false)
    });
    if !allowed || !directory.join(".kedu-test-data").is_file() {
        return Err(
            "测试必须使用 target/kedu-tests 或临时 kedu-tests 内带 .kedu-test-data 标记的目录"
                .into(),
        );
    }
    Ok(Some(directory))
}

pub fn destination_dir() -> Result<PathBuf, String> {
    if let Some(directory) = test_data_dir()? {
        return Ok(directory);
    }
    Ok(profile_dir()?.join("刻度数据"))
}

fn legacy_candidates() -> Result<Vec<(String, PathBuf)>, String> {
    let profile = profile_dir()?;
    let mut sources = vec![(
        "windows-roaming".into(),
        profile.join("AppData/Roaming/io.local.kedu/kedu.sqlite"),
    )];
    let packages = profile.join("AppData/Local/Packages");
    match fs::read_dir(&packages) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("OpenAI.Codex_") && entry.path().is_dir() {
                    sources.push((
                        format!("codex-cache:{name}"),
                        entry
                            .path()
                            .join("LocalCache/Roaming/io.local.kedu/kedu.sqlite"),
                    ));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("无法清点旧数据目录：{error}")),
    }
    sources[1..].sort_by(|left, right| left.0.cmp(&right.0));
    Ok(sources)
}

fn read_only(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("{}：{error}", path.display()))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

fn inventory(source: String, path: &Path) -> DatabaseInventory {
    let mut result = DatabaseInventory {
        source,
        requested_path: text_path(path),
        resolved_path: None,
        exists: path.exists(),
        tables: BTreeMap::new(),
        earliest_at: None,
        latest_at: None,
        error: None,
    };
    let inspected = (|| -> Result<(), String> {
        match fs::metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
            Ok(metadata) if !metadata.is_file() => return Err("数据库路径不是文件".into()),
            Ok(_) => {}
        }
        result.resolved_path = Some(text_path(&resolved_file_path(path)?));
        let connection = read_only(path)?;
        validate_database(&connection)?;
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        ).map_err(|error| error.to_string())?;
        let tables = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        for table in tables {
            let escaped = table.replace('"', "\"\"");
            let count = connection
                .query_row(&format!("SELECT COUNT(*) FROM \"{escaped}\""), [], |row| {
                    row.get(0)
                })
                .map_err(|error| error.to_string())?;
            result.tables.insert(table, count);
        }
        for table in HISTORY_TABLES {
            if !result.tables.contains_key(table) {
                continue;
            }
            let (first, last): (Option<i64>, Option<i64>) = connection
                .query_row(
                    &format!("SELECT MIN(started_at), MAX(ended_at) FROM {table}"),
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| error.to_string())?;
            if let Some(first) = first {
                result.earliest_at =
                    Some(result.earliest_at.map_or(first, |value| value.min(first)));
            }
            if let Some(last) = last {
                result.latest_at = Some(result.latest_at.map_or(last, |value| value.max(last)));
            }
        }
        Ok(())
    })();
    if let Err(error) = inspected {
        result.error = Some(error);
    }
    result
}

pub fn diagnose() -> Result<DataDiagnostics, String> {
    let package_identity = package_identity()?;
    let mut databases = legacy_candidates()?
        .into_iter()
        .map(|(source, path)| inventory(source, &path))
        .collect::<Vec<_>>();
    let destination = destination_dir()?.join(DATABASE_NAME);
    databases.push(inventory("stable-location".into(), &destination));
    let safe_to_migrate = package_identity.is_none()
        && databases.iter().all(|database| {
            database.error.is_none()
                && database.resolved_path.as_ref().is_none_or(|resolved| {
                    identity_path(Path::new(resolved))
                        == identity_path(Path::new(&database.requested_path))
                })
        });
    Ok(DataDiagnostics {
        package_identity,
        destination: text_path(&destination),
        safe_to_migrate,
        legacy_autostart_enabled: legacy_autostart_enabled()?,
        databases,
    })
}

fn validate_database(connection: &Connection) -> Result<(), String> {
    let integrity: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if integrity != "ok" {
        return Err(format!("数据库完整性检查未通过：{integrity}"));
    }
    if !table_exists(connection, "activity_segments")? {
        return Err("不是刻度数据库：缺少活动记录表".into());
    }
    Ok(())
}

fn snapshot(source: &Path, target: &Path) -> Result<(), String> {
    let source = read_only(source)?;
    let mut target = Connection::open(target).map_err(|error| error.to_string())?;
    Backup::new(&source, &mut target)
        .map_err(|error| error.to_string())?
        .run_to_completion(128, Duration::from_millis(20), None)
        .map_err(|error| error.to_string())?;
    validate_database(&target)
}

fn columns(connection: &Connection, table: &str) -> Result<BTreeSet<String>, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| error.to_string())?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(result)
}

/// Subtract authoritative history, including idle spans. Never invent parallel foreground use.
fn uncovered_intervals(start: i64, end: i64, occupied: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut cursor = start;
    let mut result = Vec::new();
    for &(left, right) in occupied {
        if right <= cursor || left >= end {
            continue;
        }
        if left > cursor {
            result.push((cursor, left.min(end)));
        }
        cursor = cursor.max(right);
        if cursor >= end {
            break;
        }
    }
    if cursor < end {
        result.push((cursor, end));
    }
    result
}

fn merge_history(
    target: &mut Connection,
    source: &Connection,
    source_name: &str,
    table: &str,
) -> Result<usize, String> {
    if !table_exists(source, table)? {
        return Ok(0);
    }
    let target_columns = columns(target, table)?;
    let shared = columns(source, table)?
        .intersection(&target_columns)
        .filter(|name| name.as_str() != "id")
        .cloned()
        .collect::<Vec<_>>();
    let start_column = shared
        .iter()
        .position(|name| name == "started_at")
        .ok_or("旧活动表缺少开始时间")?;
    let end_column = shared
        .iter()
        .position(|name| name == "ended_at")
        .ok_or("旧活动表缺少结束时间")?;
    let mut statement = source
        .prepare(&format!(
            "SELECT id,{} FROM {table} ORDER BY started_at, ended_at, id",
            shared.join(",")
        ))
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                (1..=shared.len())
                    .map(|index| row.get::<_, Value>(index))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let transaction = target.transaction().map_err(|error| error.to_string())?;
    let insert_sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        shared.join(","),
        vec!["?"; shared.len()].join(",")
    );
    let duplicate_sql = format!(
        "SELECT id FROM {table} WHERE {} LIMIT 1",
        shared
            .iter()
            .map(|name| format!("{name} IS ?"))
            .collect::<Vec<_>>()
            .join(" AND ")
    );
    let mut added = 0;
    for (source_id, mut values) in rows {
        let (Value::Integer(start), Value::Integer(end)) =
            (&values[start_column], &values[end_column])
        else {
            return Err("旧活动时间格式无效，迁移已停止".into());
        };
        let (start, end) = (*start, *end);
        let duplicate: Option<i64> = transaction
            .query_row(
                &duplicate_sql,
                rusqlite::params_from_iter(values.iter()),
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some(target_id) = duplicate {
            transaction
                .execute(
                    "INSERT INTO data_migration_sources VALUES (?1,?2,?3,?4,?5,?6,'duplicate')",
                    params![source_name, table, source_id, target_id, start, end],
                )
                .map_err(|error| error.to_string())?;
            continue;
        }
        if end < start {
            return Err("旧活动结束时间早于开始时间，迁移已停止".into());
        }
        if end == start {
            transaction
                .execute(&insert_sql, rusqlite::params_from_iter(values.iter()))
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT INTO data_migration_sources VALUES (?1,?2,?3,?4,?5,?6,'added-zero')",
                    params![
                        source_name,
                        table,
                        source_id,
                        transaction.last_insert_rowid(),
                        start,
                        end
                    ],
                )
                .map_err(|error| error.to_string())?;
            added += 1;
            continue;
        }
        let occupied = {
            let mut query = transaction.prepare(&format!("SELECT started_at,ended_at FROM {table} WHERE ended_at>?1 AND started_at<?2 ORDER BY started_at,ended_at"))
                .map_err(|error| error.to_string())?;
            let rows = query
                .query_map(params![start, end], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            rows
        };
        let pieces = uncovered_intervals(start, end, &occupied);
        if pieces.is_empty() {
            transaction.execute("INSERT INTO data_migration_sources VALUES (?1,?2,?3,NULL,?4,?5,'primary-overlap')", params![source_name,table,source_id,start,end])
                .map_err(|error| error.to_string())?;
        }
        for (left, right) in pieces {
            values[start_column] = Value::Integer(left);
            values[end_column] = Value::Integer(right);
            transaction
                .execute(&insert_sql, rusqlite::params_from_iter(values.iter()))
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT INTO data_migration_sources VALUES (?1,?2,?3,?4,?5,?6,'added')",
                    params![
                        source_name,
                        table,
                        source_id,
                        transaction.last_insert_rowid(),
                        left,
                        right
                    ],
                )
                .map_err(|error| error.to_string())?;
            added += 1;
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(added)
}

fn migrate_sources(directory: &Path, sources: &[(String, PathBuf)]) -> Result<PathBuf, String> {
    let destination = directory.join(DATABASE_NAME);
    if destination.exists() {
        validate_database(&read_only(&destination)?)?;
        return Ok(destination);
    }
    if let Some((_, primary)) = sources
        .iter()
        .find(|(source, _)| source == "windows-roaming")
    {
        let secondary_exists = sources
            .iter()
            .any(|(source, path)| source != "windows-roaming" && path.is_file());
        if !primary.is_file() && secondary_exists {
            return Err("只找到隔离环境中的旧数据，无法核实正常 Windows 用户库；迁移已停止，现有文件没有改动".into());
        }
    }
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let migration_id = format!(
        "{}-{}",
        chrono::Utc::now().timestamp_millis(),
        std::process::id()
    );
    let recovery = directory.join("recovery").join(&migration_id);
    fs::create_dir_all(&recovery).map_err(|error| error.to_string())?;
    let mut snapshots = Vec::new();
    let mut inventories = Vec::new();
    for (index, (name, path)) in sources.iter().enumerate() {
        let before = inventory(name.clone(), path);
        if let Some(error) = before.error {
            return Err(error);
        }
        if !before.exists {
            continue;
        }
        let backup_path = recovery.join(format!("source-{index}.sqlite"));
        snapshot(path, &backup_path)?;
        // Counts come from the consistent backup, with the original physical identity retained.
        let mut saved = inventory(name.clone(), &backup_path);
        saved.requested_path = before.requested_path;
        saved.resolved_path = before.resolved_path;
        inventories.push(saved);
        snapshots.push((name.clone(), backup_path));
    }
    let staged = recovery.join("merged.sqlite");
    if let Some((_, primary)) = snapshots.first() {
        snapshot(primary, &staged)?;
    }
    Storage::new(staged.clone())?;
    let mut target = Connection::open(&staged).map_err(|error| error.to_string())?;
    let preserved_autostart = if snapshots.is_empty() {
        None
    } else {
        let existing = target
            .query_row(
                "SELECT value FROM app_settings WHERE key='autostart_enabled'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let enabled = existing
            .map(|value| value == "true")
            .map_or_else(legacy_autostart_enabled, Ok)?;
        target
            .execute(
                "INSERT OR REPLACE INTO app_settings(key,value) VALUES('autostart_enabled',?1)",
                [enabled.to_string()],
            )
            .map_err(|error| error.to_string())?;
        Some(enabled)
    };
    target.execute_batch("CREATE TABLE IF NOT EXISTS data_migration_sources (source TEXT NOT NULL, source_table TEXT NOT NULL, source_id INTEGER NOT NULL, target_id INTEGER, started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL, outcome TEXT NOT NULL); CREATE TABLE IF NOT EXISTS data_migration_manifest (id INTEGER PRIMARY KEY CHECK(id=1), manifest TEXT NOT NULL);")
        .map_err(|error| error.to_string())?;
    let mut added_segments = BTreeMap::new();
    for (source_name, path) in snapshots.iter().skip(1) {
        let source = read_only(path)?;
        for table in HISTORY_TABLES {
            let added = merge_history(&mut target, &source, source_name, table)?;
            *added_segments.entry(table.to_string()).or_insert(0) += added;
        }
    }
    let manifest = MigrationManifest {
        format_version: 1,
        created_at: chrono::Utc::now().timestamp_millis(),
        destination: text_path(&destination),
        primary_source: snapshots.first().map(|(name, _)| name.clone()),
        sources: inventories,
        recovery_directory: text_path(&recovery),
        added_segments,
        preserved_configurations: snapshots
            .iter()
            .skip(1)
            .map(|(_, path)| text_path(path))
            .collect(),
        preserved_autostart,
    };
    let manifest_json =
        serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
    target
        .execute(
            "INSERT INTO data_migration_manifest (id,manifest) VALUES (1,?1)",
            [&manifest_json],
        )
        .map_err(|error| error.to_string())?;
    validate_database(&target)?;
    let violation: Option<String> = target
        .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some(table) = violation {
        return Err(format!("迁移关联校验失败：{table}"));
    }
    target
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
        .map_err(|error| error.to_string())?;
    drop(target);
    fs::write(recovery.join("manifest.json"), &manifest_json).map_err(|error| error.to_string())?;
    // A rename on the same volume publishes only a complete, closed SQLite file.
    fs::rename(&staged, &destination)
        .map_err(|error| format!("无法启用新数据，旧数据仍保留：{error}"))?;
    Ok(destination)
}

#[cfg(windows)]
fn other_kedu_process_running() -> Result<bool, String> {
    use std::ffi::c_void;
    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        process_id: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        priority: i32,
        flags: u32,
        executable: [u16; 260],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Process32FirstW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    let snapshot = unsafe { CreateToolhelp32Snapshot(2, 0) };
    if snapshot as isize == -1 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
    entry.size = std::mem::size_of::<ProcessEntry>() as u32;
    let mut valid = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    let mut found = false;
    while valid {
        let length = entry
            .executable
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(260);
        let name = String::from_utf16_lossy(&entry.executable[..length]);
        if name.eq_ignore_ascii_case("kedu.exe") && entry.process_id != std::process::id() {
            found = true;
            break;
        }
        valid = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe {
        CloseHandle(snapshot);
    }
    Ok(found)
}

#[cfg(not(windows))]
fn other_kedu_process_running() -> Result<bool, String> {
    Ok(false)
}

pub fn prepare_database() -> Result<PathBuf, String> {
    let directory = destination_dir()?;
    let destination = directory.join(DATABASE_NAME);
    if test_data_dir()?.is_some() {
        return Ok(destination);
    }
    if destination.exists() {
        if identity_path(&resolved_file_path(&destination)?) != identity_path(&destination) {
            return Err("数据路径发生重定向，已停止打开数据库".into());
        }
        validate_database(&read_only(&destination)?)?;
        return Ok(destination);
    }
    let diagnostics = diagnose()?;
    if !diagnostics.safe_to_migrate {
        return Err("尚未核实正常 Windows 环境中的旧数据。请从开始菜单或资源管理器打开刻度完成迁移；旧数据没有改动。".into());
    }
    if other_kedu_process_running()? {
        return Err("请先通过托盘正常退出旧版刻度，再打开新版完成数据迁移。".into());
    }
    migrate_sources(&directory, &legacy_candidates()?)
}

/// Runs before Tauri/single-instance startup. Diagnostics never initialize a database.
pub fn handle_cli() -> Option<Result<(), String>> {
    let arguments = std::env::args().collect::<Vec<_>>();
    let diagnose_requested = arguments
        .iter()
        .any(|argument| argument == "--diagnose-data");
    let migrate_requested = arguments
        .iter()
        .any(|argument| argument == "--migrate-data");
    if !diagnose_requested && !migrate_requested {
        return None;
    }
    Some((|| {
        let result = if migrate_requested {
            let path = prepare_database()?;
            serde_json::json!({"ok":true,"databasePath":path,"diagnostics":diagnose()?})
        } else {
            serde_json::to_value(diagnose()?).map_err(|error| error.to_string())?
        };
        let output = serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?;
        if let Some(index) = arguments.iter().position(|argument| argument == "--output") {
            let path = arguments.get(index + 1).ok_or("--output 缺少文件路径")?;
            fs::write(path, &output).map_err(|error| error.to_string())?;
        } else {
            println!("{output}");
        }
        Ok(())
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join("kedu-tests").join(format!(
                "migration-{}-{}-{id}",
                std::process::id(),
                chrono::Utc::now().timestamp_millis()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn database(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            Storage::new(path.clone()).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn insert(path: &Path, start: i64, end: i64, name: &str) {
        Connection::open(path).unwrap().execute("INSERT INTO activity_segments (started_at,ended_at,app_name,process_path,title,idle) VALUES (?1,?2,?3,'test.exe','test',0)",params![start,end,name]).unwrap();
    }
    #[test]
    fn subtracts_overlapping_authoritative_intervals() {
        assert_eq!(
            uncovered_intervals(0, 100, &[(20, 40), (30, 60), (80, 120)]),
            vec![(0, 20), (60, 80)]
        );
        assert!(uncovered_intervals(20, 40, &[(0, 80)]).is_empty());
        assert!(uncovered_intervals(40, 20, &[]).is_empty());
    }
    #[test]
    fn migration_preserves_primary_and_only_adds_unique_nonoverlapping_history() {
        let fixture = Fixture::new();
        let primary = fixture.database("primary.sqlite");
        let secondary = fixture.database("secondary.sqlite");
        insert(&primary, 20, 60, "primary");
        insert(&secondary, 20, 60, "primary");
        insert(&secondary, 0, 100, "secondary");
        Connection::open(&secondary)
            .unwrap()
            .execute(
                "INSERT INTO web_activity_segments
                 (started_at,ended_at,domain,title,media_active,idle,idle_exempt)
                 VALUES (30,30,'example.com','legacy event',0,0,0)",
                [],
            )
            .unwrap();
        Connection::open(&primary)
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO app_settings VALUES ('migration-test','primary'), ('autostart_enabled','false')",
                [],
            )
            .unwrap();
        Connection::open(&secondary)
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO app_settings VALUES ('migration-test','secondary'), ('autostart_enabled','true')",
                [],
            )
            .unwrap();
        let sources = vec![
            ("normal".into(), primary.clone()),
            ("cache".into(), secondary.clone()),
        ];
        let target = migrate_sources(&fixture.0.join("destination"), &sources).unwrap();
        let connection = read_only(&target).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM activity_segments", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT SUM(ended_at-started_at) FROM activity_segments",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            100
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM web_activity_segments", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM data_migration_sources WHERE outcome='added-zero'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key='migration-test'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "primary"
        );
        // Migration tests use explicit settings instead of the user's registry.
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key='autostart_enabled'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "false"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM data_migration_sources WHERE outcome='duplicate'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            migrate_sources(&fixture.0.join("destination"), &sources).unwrap(),
            target
        );
        assert_eq!(
            inventory("original".into(), &primary).tables["activity_segments"],
            1
        );
        assert_eq!(
            inventory("original".into(), &secondary).tables["activity_segments"],
            2
        );
    }

    #[test]
    fn isolated_copy_never_substitutes_for_a_missing_windows_primary() {
        let fixture = Fixture::new();
        let secondary = fixture.database("secondary.sqlite");
        insert(&secondary, 0, 100, "secondary");
        let destination = fixture.0.join("destination");
        let error = migrate_sources(
            &destination,
            &[
                ("windows-roaming".into(), fixture.0.join("missing.sqlite")),
                ("codex-cache:test".into(), secondary),
            ],
        )
        .unwrap_err();
        assert!(error.contains("无法核实正常 Windows 用户库"));
        assert!(!destination.join(DATABASE_NAME).exists());
    }
    #[test]
    fn corrupt_source_never_publishes_an_empty_destination() {
        let fixture = Fixture::new();
        let invalid = fixture.0.join("invalid.sqlite");
        fs::write(&invalid, b"not a sqlite database").unwrap();
        let destination = fixture.0.join("destination");
        assert!(migrate_sources(&destination, &[("normal".into(), invalid)]).is_err());
        assert!(!destination.join(DATABASE_NAME).exists());
    }
    #[test]
    fn snapshot_includes_uncheckpointed_wal_records() {
        let fixture = Fixture::new();
        let source = fixture.database("wal.sqlite");
        let writer = Connection::open(&source).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        writer.execute("INSERT INTO activity_segments (started_at,ended_at,app_name,process_path,title,idle) VALUES (0,100,'wal','wal.exe','',0)",[]).unwrap();
        let backup = fixture.0.join("snapshot.sqlite");
        snapshot(&source, &backup).unwrap();
        assert_eq!(
            inventory("snapshot".into(), &backup).tables["activity_segments"],
            1
        );
    }
}
