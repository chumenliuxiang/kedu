//! Category definitions and defaults share one transaction; activity rows stay immutable.
use super::*;
use crate::category_models::*;

pub(crate) const DEFAULT_COLOR: &str = "#32776f";
pub(crate) fn valid_color(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
pub(crate) fn validate_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty()
        || name == "未分类"
        || name.chars().count() > 60
        || name.chars().any(char::is_control)
    {
        return Err("分类名称需为 1–60 个字符，且不能使用“未分类”".into());
    }
    Ok(name)
}
pub(crate) fn migrate(conn: &Connection, version: i64) -> Result<(), String> {
    if version >= 6 {
        validate(conn)?;
    }
    conn.execute_batch("CREATE TABLE IF NOT EXISTS stat_categories(id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL COLLATE NOCASE UNIQUE,color TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS classification_exports(id TEXT PRIMARY KEY,snapshot TEXT NOT NULL,created_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS classification_undo(id INTEGER PRIMARY KEY CHECK(id=1),payload TEXT NOT NULL);").map_err(|e| e.to_string())?;
    let mut statement = conn.prepare("SELECT DISTINCT trim(category) FROM (SELECT category FROM classification_rules UNION SELECT category FROM statistics_defaults UNION SELECT category FROM statistics_overrides) WHERE trim(category)<>'' AND trim(category)<>'未分类'").map_err(|e| e.to_string())?;
    let names = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for name in names {
        conn.execute(
            "INSERT OR IGNORE INTO stat_categories(name,color) VALUES(?1,?2)",
            params![name, DEFAULT_COLOR],
        )
        .map_err(|e| e.to_string())?;
    }
    // Old detail and rule editors remain supported, including backup imports.
    for table in [
        "classification_rules",
        "statistics_defaults",
        "statistics_overrides",
    ] {
        if version < 6 {
            // Preserve revision timestamps and rule priority while unifying old
            // case/whitespace variants into the same display category.
            conn.execute_batch(&format!("UPDATE {table} SET category=(SELECT name FROM stat_categories WHERE name=trim({table}.category) COLLATE NOCASE) WHERE EXISTS(SELECT 1 FROM stat_categories WHERE name=trim({table}.category) COLLATE NOCASE);")).map_err(|e|e.to_string())?;
        }
        for event in ["INSERT", "UPDATE OF category"] {
            let suffix = if event == "INSERT" {
                "insert"
            } else {
                "update"
            };
            conn.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS category_{table}_{suffix} AFTER {event} ON {table}
                WHEN trim(NEW.category)<>'' AND trim(NEW.category)<>'未分类' BEGIN
                INSERT INTO stat_categories(name,color) SELECT trim(NEW.category),'{DEFAULT_COLOR}' WHERE NOT EXISTS(SELECT 1 FROM stat_categories WHERE name=trim(NEW.category) COLLATE NOCASE);
                UPDATE {table} SET category=(SELECT name FROM stat_categories WHERE name=trim(NEW.category) COLLATE NOCASE) WHERE rowid=NEW.rowid;
                END;")).map_err(|e|e.to_string())?;
        }
    }
    Ok(())
}
pub(crate) fn validate(conn: &Connection) -> Result<(), String> {
    conn.prepare("SELECT id,name,color FROM stat_categories")
        .map_err(|e| format!("备份缺少分类目录：{e}"))?;
    conn.prepare("SELECT id,snapshot,created_at FROM classification_exports")
        .map_err(|e| e.to_string())?;
    conn.prepare("SELECT id,payload FROM classification_undo")
        .map_err(|e| e.to_string())?;
    for category in list_categories(conn)? {
        if category.name.trim().is_empty()
            || category.name == "未分类"
            || !valid_color(&category.color)
        {
            return Err("备份中的分类目录无效".into());
        }
    }
    Ok(())
}
pub(crate) fn list_categories(conn: &Connection) -> Result<Vec<StatCategory>, String> {
    let mut query = conn
        .prepare("SELECT id,name,color FROM stat_categories ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map([], |r| {
            Ok(StatCategory {
                id: r.get(0)?,
                name: r.get(1)?,
                color: r.get(2)?,
                application_count: 0,
                website_count: 0,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}
pub(crate) fn ensure_category(conn: &Connection, name: &str, color: &str) -> Result<i64, String> {
    let name = validate_name(name)?;
    if !valid_color(color) {
        return Err("请选择有效分类颜色".into());
    }
    if let Some(existing) = list_categories(conn)?
        .into_iter()
        .find(|c| c.name.to_lowercase() == name.to_lowercase())
    {
        return Ok(existing.id);
    }
    conn.execute(
        "INSERT INTO stat_categories(name,color) VALUES(?1,?2)",
        params![name, color],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}
fn category_by_id(conn: &Connection, id: Option<i64>) -> Result<String, String> {
    match id.filter(|id| *id != 0) {
        None => Ok("未分类".into()),
        Some(id) => conn
            .query_row("SELECT name FROM stat_categories WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "分类已不存在，请刷新".into()),
    }
}
pub(crate) fn next_revision(conn: &Connection) -> Result<i64, String> {
    let last:i64=conn.query_row("SELECT COALESCE(MAX(updated_at),0) FROM (SELECT updated_at FROM statistics_defaults UNION ALL SELECT updated_at FROM classification_rules UNION ALL SELECT updated_at FROM statistics_overrides UNION ALL SELECT updated_at FROM timeline_annotations)",[],|r|r.get(0)).map_err(|e|e.to_string())?;
    Ok(Utc::now().timestamp_millis().max(last.saturating_add(1)))
}
pub(crate) fn catalog_entries(conn: &Connection) -> Result<Vec<ClassificationItem>, String> {
    let mut entries = BTreeMap::<String, ClassificationItem>::new();
    let mut query=conn.prepare("SELECT app_name,process_path,MAX(ended_at) FROM activity_segments GROUP BY lower(process_path),CASE WHEN process_path='' THEN app_name ELSE '' END ORDER BY MAX(ended_at) DESC").map_err(|e|e.to_string())?;
    let apps = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for (name, path, last_seen_at) in apps {
        let target =
            crate::app_identity::canonical_executable(if path.is_empty() { &name } else { &path });
        let name = crate::app_identity::display_name(&target, &name);
        if target == ".exe" {
            continue;
        }
        let key = format!("application:{target}");
        entries.entry(key.clone()).or_insert(ClassificationItem {
            key,
            kind: "application".into(),
            target,
            name,
            process_path: (!path.is_empty()).then_some(path),
            category_id: None,
            category_name: "未分类".into(),
            last_seen_at,
            milliseconds: 0,
        });
    }
    let mut query=conn.prepare("SELECT domain,MAX(ended_at) FROM web_activity_segments WHERE domain<>'' GROUP BY domain ORDER BY MAX(ended_at) DESC").map_err(|e|e.to_string())?;
    let sites = query
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for (domain, last_seen_at) in sites {
        let (target, name) = web_recording::site_identity(&domain);
        let key = format!("website:{target}");
        entries.entry(key.clone()).or_insert(ClassificationItem {
            key,
            kind: "website".into(),
            target,
            name,
            process_path: None,
            category_id: None,
            category_name: "未分类".into(),
            last_seen_at,
            milliseconds: 0,
        });
    }
    let categories = list_categories(conn)?;
    let mut query=conn.prepare("SELECT target_kind,target_pattern,category,updated_at,id FROM classification_rules WHERE enabled=1 AND target_kind<>'title' UNION ALL SELECT target_kind,target,category,updated_at,0 FROM statistics_defaults ORDER BY updated_at DESC,id DESC").map_err(|e|e.to_string())?;
    let rules = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?.to_lowercase(),
                r.get::<_, String>(2)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for item in entries.values_mut() {
        if let Some((_, _, category, _)) = rules.iter().find(|(kind, pattern, _, id)| {
            if kind != &item.kind {
                return false;
            }
            if *id == 0 {
                return item.target
                    == if kind == "application" {
                        crate::app_identity::canonical_executable(pattern)
                    } else {
                        pattern.clone()
                    };
            }
            if kind == "website" {
                return domain_matches(&item.target, pattern);
            }
            crate::app_identity::matches(&item.target, &item.name, pattern)
        }) {
            item.category_name = category.clone();
            item.category_id = categories
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(category))
                .map(|c| c.id);
        }
    }
    Ok(entries.into_values().collect())
}
pub(crate) fn catalog_items(
    conn: &Connection,
    now: i64,
) -> Result<Vec<ClassificationItem>, String> {
    let mut items = catalog_entries(conn)?;
    let usage = Storage::classification_usage_on(conn, now)?;
    for item in &mut items {
        item.milliseconds = *usage.get(&item.key).unwrap_or(&0);
    }
    Ok(items)
}
pub(crate) fn assign_in_connection(
    conn: &Connection,
    keys: &[String],
    category_id: Option<i64>,
) -> Result<(), String> {
    if keys.is_empty() || keys.len() > 10000 {
        return Err("请选择 1–10000 个项目".into());
    }
    let category = category_by_id(conn, category_id)?;
    let known = catalog_entries(conn)?
        .into_iter()
        .map(|i| (i.key, (i.kind, i.target)))
        .collect::<BTreeMap<_, _>>();
    if keys.iter().any(|key| !known.contains_key(key)) {
        return Err("清单包含未记录的应用或网站，请刷新".into());
    }
    let revision = next_revision(conn)?;
    for key in keys.iter().collect::<BTreeSet<_>>() {
        let (kind, target) = &known[key];
        conn.execute("INSERT INTO statistics_defaults(target_kind,target,category,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(target_kind,target) DO UPDATE SET category=excluded.category,updated_at=excluded.updated_at",params![kind,target,category,revision]).map_err(|e|e.to_string())?;
    }
    Ok(())
}
impl Storage {
    pub fn classification_workspace(
        &self,
        query: ClassificationQuery,
    ) -> Result<ClassificationWorkspace, String> {
        if !matches!(query.kind.as_str(), "application" | "website") {
            return Err("未知分类目标类型".into());
        }
        let conn = self.connection()?;
        conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;
        let all = catalog_items(&conn, Utc::now().timestamp_millis())?;
        let mut categories = list_categories(&conn)?;
        for category in &mut categories {
            category.application_count = all
                .iter()
                .filter(|i| i.kind == "application" && i.category_id == Some(category.id))
                .count();
            category.website_count = all
                .iter()
                .filter(|i| i.kind == "website" && i.category_id == Some(category.id))
                .count();
        }
        let unclassified_count = all
            .iter()
            .filter(|i| i.kind == query.kind && i.category_id.is_none())
            .count();
        let search = query.search.unwrap_or_default().trim().to_lowercase();
        let recent = Utc::now().timestamp_millis() - 30 * 86400000;
        let mut items = all
            .into_iter()
            .filter(|i| {
                i.kind == query.kind
                    && (!query.recent_only.unwrap_or(false) || i.last_seen_at >= recent)
                    && (query.category_id.is_none()
                        || i.category_id.unwrap_or(0) == query.category_id.unwrap())
                    && (query.status.as_deref() != Some("classified") || i.category_id.is_some())
                    && (query.status.as_deref() != Some("unclassified") || i.category_id.is_none())
                    && (search.is_empty()
                        || format!("{} {}", i.name, i.target)
                            .to_lowercase()
                            .contains(&search))
            })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| {
            a.category_id
                .is_some()
                .cmp(&b.category_id.is_some())
                .then(b.milliseconds.cmp(&a.milliseconds))
                .then(b.last_seen_at.cmp(&a.last_seen_at))
                .then(a.key.cmp(&b.key))
        });
        let total = items.len();
        let offset = query.offset.unwrap_or(0);
        let limit = query.limit.unwrap_or(30).clamp(1, 100);
        let items = items.into_iter().skip(offset).take(limit).collect();
        let undo_available = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM classification_undo)",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(ClassificationWorkspace {
            categories,
            items,
            total,
            offset,
            limit,
            unclassified_count,
            undo_available,
        })
    }
    pub fn save_stat_category(&self, input: NewStatCategory) -> Result<StatCategory, String> {
        let name = validate_name(&input.name)?;
        if !valid_color(&input.color) {
            return Err("请选择有效颜色".into());
        }
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let categories = list_categories(&tx)?;
        if categories
            .iter()
            .any(|c| c.name.to_lowercase() == name.to_lowercase() && Some(c.id) != input.id)
        {
            return Err("已有同名分类".into());
        }
        let id = if let Some(id) = input.id {
            let old = categories
                .iter()
                .find(|c| c.id == id)
                .ok_or("分类已不存在，请刷新")?;
            tx.execute(
                "UPDATE stat_categories SET name=?1,color=?2 WHERE id=?3",
                params![name, input.color, id],
            )
            .map_err(|e| e.to_string())?;
            for table in [
                "classification_rules",
                "statistics_defaults",
                "statistics_overrides",
                "timeline_annotations",
            ] {
                tx.execute(
                    &format!("UPDATE {table} SET category=?1 WHERE category=?2 COLLATE NOCASE"),
                    params![name, old.name],
                )
                .map_err(|e| e.to_string())?;
            }
            id
        } else {
            ensure_category(&tx, name, &input.color)?
        };
        let result = list_categories(&tx)?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or("无法读取分类")?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result)
    }
    pub fn remove_stat_category(&self, id: i64, destination_id: Option<i64>) -> Result<(), String> {
        if destination_id == Some(id) {
            return Err("请选择另一个分类".into());
        }
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let old = category_by_id(&tx, Some(id))?;
        let destination = category_by_id(&tx, destination_id)?;
        for table in [
            "classification_rules",
            "statistics_defaults",
            "statistics_overrides",
            "timeline_annotations",
        ] {
            tx.execute(
                &format!("UPDATE {table} SET category=?1 WHERE category=?2 COLLATE NOCASE"),
                params![destination, old],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute("DELETE FROM stat_categories WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn assign_stat_categories(
        &self,
        keys: Vec<String>,
        category_id: Option<i64>,
    ) -> Result<(), String> {
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        assign_in_connection(&tx, &keys, category_id)?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn create_common_stat_categories(&self) -> Result<Vec<StatCategory>, String> {
        let mut conn = self.connection()?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        for (name, color) in [
            ("学习", "#32776f"),
            ("工作", "#537da2"),
            ("娱乐", "#ad7b41"),
            ("工具", "#9873a5"),
        ] {
            ensure_category(&tx, name, color)?;
        }
        let result = list_categories(&tx)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result)
    }
}
