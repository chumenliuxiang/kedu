use super::*;
use crate::{category_models::*, domain::StatisticsQuery};
fn store() -> Storage {
    let path = std::env::temp_dir().join("kedu-tests").join(format!(
        "classification-{}-{}.sqlite",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap()
    ));
    Storage::new(path).unwrap()
}
fn app(s: &Storage, name: &str, exe: &str, start: i64, end: i64) -> i64 {
    let c = s.connection().unwrap();
    c.execute("INSERT INTO activity_segments(started_at,ended_at,app_name,process_path,title,idle) VALUES(?1,?2,?3,?4,'private document title',0)",params![start,end,name,format!("C:\\private\\{exe}")]).unwrap();
    c.last_insert_rowid()
}
fn category(s: &Storage, name: &str) -> StatCategory {
    s.save_stat_category(NewStatCategory {
        id: None,
        name: name.into(),
        color: "#123456".into(),
    })
    .unwrap()
}
fn workspace(s: &Storage, kind: &str) -> ClassificationWorkspace {
    s.classification_workspace(ClassificationQuery {
        kind: kind.into(),
        limit: Some(100),
        ..Default::default()
    })
    .unwrap()
}
fn query(s: &Storage, start: i64, end: i64) -> ActivityStatistics {
    s.query_statistics_with_options(
        StatisticsQuery {
            start,
            end,
            bucket: "day".into(),
            target_kind: None,
            target: None,
            offset: None,
            limit: None,
            search: None,
            include_excluded: None,
            page_key: None,
        },
        false,
        false,
    )
    .unwrap()
}
fn export_document(s: &Storage, keys: &[&str], assignments: serde_json::Value) -> (String, String) {
    let exported = s
        .export_classification_package(keys.iter().map(|k| k.to_string()).collect())
        .unwrap();
    let data: serde_json::Value = serde_json::from_str(
        exported
            .content
            .split("```json\n")
            .nth(1)
            .unwrap()
            .split("\n```")
            .next()
            .unwrap(),
    )
    .unwrap();
    let document=serde_json::json!({"version":1,"exportId":data["exportId"],"categories":[{"name":"学习","color":"#32776f"},{"name":"工作","color":"#537da2"}],"assignments":assignments}).to_string();
    (exported.content, document)
}
#[test]
fn category_catalog_is_complete_and_uses_latest_executable_path() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    for n in 0..105 {
        app(
            &s,
            &format!("App{n}"),
            &format!("app{n}.exe"),
            now - 500000 + n * 1000,
            now - 499001 + n * 1000,
        );
    }
    app(&s, "New name", "app0.exe", now - 1000, now);
    let first = workspace(&s, "application");
    assert_eq!(first.total, 105);
    assert_eq!(first.items.len(), 100);
    assert_eq!(first.categories.len(), 0);
    assert_eq!(first.unclassified_count, 105);
    assert!(first.items.iter().any(|i| i.name == "New name"));
    let next = s
        .classification_workspace(ClassificationQuery {
            kind: "application".into(),
            offset: Some(100),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(next.items.len(), 5);
}
#[test]
fn default_assignment_preserves_title_and_segment_priority_and_raw_history() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    let id = app(&s, "Editor", "code.exe", now - 2000, now);
    let learn = category(&s, "学习");
    s.assign_stat_categories(vec!["application:code.exe".into()], Some(learn.id))
        .unwrap();
    let summary = query(&s, now - 3000, now);
    assert_eq!(summary.category_usage[0].app_name, "学习");
    assert_eq!(summary.category_usage[0].color, "#123456");
    s.create_classification_rule(NewClassificationRule {
        name: "标题".into(),
        target_kind: "title".into(),
        target_pattern: "private".into(),
        category: "阅读".into(),
    })
    .unwrap();
    assert_eq!(
        query(&s, now - 3000, now).category_usage[0].app_name,
        "阅读"
    );
    s.set_activity_category("application", id, Some("手动"))
        .unwrap();
    assert_eq!(
        query(&s, now - 3000, now).category_usage[0].app_name,
        "手动"
    );
    assert_eq!(workspace(&s, "application").items[0].category_name, "学习");
    assert_eq!(
        s.connection()
            .unwrap()
            .query_row(
                "SELECT title FROM activity_segments WHERE id=?1",
                [id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "private document title"
    );
}
#[test]
fn category_rename_merge_delete_update_all_references() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    let id = app(&s, "Editor", "code.exe", now - 2000, now);
    let cat = category(&s, "办公");
    s.assign_stat_categories(vec!["application:code.exe".into()], Some(cat.id))
        .unwrap();
    s.set_activity_category("application", id, Some("办公"))
        .unwrap();
    s.create_classification_rule(NewClassificationRule {
        name: "标题".into(),
        target_kind: "title".into(),
        target_pattern: "private".into(),
        category: "办公".into(),
    })
    .unwrap();
    s.save_stat_category(NewStatCategory {
        id: Some(cat.id),
        name: "工作".into(),
        color: "#abcdef".into(),
    })
    .unwrap();
    assert_eq!(
        query(&s, now - 3000, now).category_usage[0].app_name,
        "工作"
    );
    assert_eq!(s.list_classification_rules().unwrap()[0].category, "工作");
    let dest = category(&s, "学习");
    s.remove_stat_category(cat.id, Some(dest.id)).unwrap();
    assert_eq!(
        query(&s, now - 3000, now).category_usage[0].app_name,
        "学习"
    );
    s.remove_stat_category(dest.id, None).unwrap();
    assert_eq!(
        query(&s, now - 3000, now).category_usage[0].app_name,
        "未分类"
    );
    assert!(workspace(&s, "application").categories.is_empty());
}
#[test]
fn category_validation_and_batch_failure_are_atomic() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    app(&s, "Editor", "code.exe", now - 1000, now);
    let cat = category(&s, "Work");
    assert!(s
        .save_stat_category(NewStatCategory {
            id: None,
            name: " work ".into(),
            color: "#123456".into()
        })
        .is_err());
    assert!(s
        .save_stat_category(NewStatCategory {
            id: None,
            name: "未分类".into(),
            color: "#123456".into()
        })
        .is_err());
    assert!(s
        .save_stat_category(NewStatCategory {
            id: None,
            name: "x".into(),
            color: "red".into()
        })
        .is_err());
    assert!(s
        .assign_stat_categories(
            vec![
                "application:code.exe".into(),
                "application:unknown.exe".into()
            ],
            Some(cat.id)
        )
        .is_err());
    assert_eq!(workspace(&s, "application").unclassified_count, 1);
    assert!(s.remove_stat_category(cat.id, Some(cat.id)).is_err());
}
#[test]
fn websites_group_by_parent_and_duration_is_browser_subset() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    app(&s, "Edge", "msedge.exe", now - 10000, now);
    let c = s.connection().unwrap();
    for (domain, a, b) in [
        ("www.bilibili.com", now - 9000, now - 5000),
        ("live.bilibili.com", now - 5000, now - 2000),
    ] {
        c.execute("INSERT INTO web_activity_segments(started_at,ended_at,domain,title,idle,browser_target) VALUES(?1,?2,?3,'private video',0,'msedge.exe')",params![a,b,domain]).unwrap();
    }
    let sites = workspace(&s, "website");
    assert_eq!(sites.total, 1);
    assert_eq!(sites.items[0].target, "bilibili.com");
    assert_eq!(sites.items[0].milliseconds, 7000);
    let cat = category(&s, "学习");
    s.assign_stat_categories(vec!["website:bilibili.com".into()], Some(cat.id))
        .unwrap();
    let stats = query(&s, now - 10000, now);
    assert_eq!(stats.effective_milliseconds, 10000);
    assert_eq!(
        stats
            .category_usage
            .iter()
            .find(|i| i.app_name == "学习")
            .unwrap()
            .milliseconds,
        7000
    );
}
#[test]
fn export_omits_sensitive_fields_and_only_includes_selected_history() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    app(&s, "Editor", "code.exe", now - 1000, now);
    app(&s, "Other", "other.exe", now - 2000, now - 1000);
    let (content, _) = export_document(
        &s,
        &["application:code.exe"],
        serde_json::json!([{"key":"application:code.exe","category":"学习"}]),
    );
    assert!(!content.contains("C:\\"));
    assert!(!content.contains("private document"));
    assert!(!content.contains("other.exe"));
    assert!(content.contains("last30DaysMilliseconds"));
    assert!(content.contains("先问"));
}
#[test]
fn import_selection_preview_and_undo_survive_restart() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    app(&s, "Editor", "code.exe", now - 1000, now);
    let (_, doc) = export_document(
        &s,
        &["application:code.exe"],
        serde_json::json!([{"key":"application:code.exe","category":"学习"}]),
    );
    let p = s.preview_classification_import(doc.clone()).unwrap();
    assert!(p.rows[0].selected);
    let result = s
        .apply_classification_import(doc, vec!["application:code.exe".into()], p.token)
        .unwrap();
    assert_eq!(result.applied, 1);
    assert_eq!(result.created_categories, 1);
    let reopened = Storage::new(s.path.clone()).unwrap();
    assert_eq!(
        workspace(&reopened, "application").items[0].category_name,
        "学习"
    );
    assert!(workspace(&reopened, "application").undo_available);
    assert_eq!(reopened.undo_classification_import().unwrap().restored, 1);
    assert_eq!(workspace(&reopened, "application").unclassified_count, 1);
    assert!(workspace(&reopened, "application").categories.is_empty());
}
#[test]
fn later_manual_assignment_is_not_undone_and_unselected_items_stay() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    app(&s, "Editor", "code.exe", now - 2000, now - 1000);
    app(&s, "Other", "other.exe", now - 1000, now);
    let (_, doc) = export_document(
        &s,
        &["application:code.exe", "application:other.exe"],
        serde_json::json!([{"key":"application:code.exe","category":"学习"},{"key":"application:other.exe","category":"学习"}]),
    );
    let p = s.preview_classification_import(doc.clone()).unwrap();
    s.apply_classification_import(doc, vec!["application:code.exe".into()], p.token)
        .unwrap();
    let work = category(&s, "工作");
    s.assign_stat_categories(vec!["application:code.exe".into()], Some(work.id))
        .unwrap();
    let undo = s.undo_classification_import().unwrap();
    assert_eq!(undo.skipped, 1);
    assert_eq!(workspace(&s, "application").unclassified_count, 1);
    assert!(workspace(&s, "application")
        .items
        .iter()
        .any(|i| i.category_name == "工作"));
}
#[test]
fn stale_export_and_preview_never_silently_overwrite() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    app(&s, "Editor", "code.exe", now - 1000, now);
    let work = category(&s, "工作");
    let (_, doc) = export_document(
        &s,
        &["application:code.exe"],
        serde_json::json!([{"key":"application:code.exe","category":"学习"}]),
    );
    let before = s.preview_classification_import(doc.clone()).unwrap();
    s.assign_stat_categories(vec!["application:code.exe".into()], Some(work.id))
        .unwrap();
    assert!(s
        .apply_classification_import(
            doc.clone(),
            vec!["application:code.exe".into()],
            before.token
        )
        .is_err());
    let after = s.preview_classification_import(doc.clone()).unwrap();
    assert!(!after.rows[0].selected);
    assert_eq!(after.rows[0].status, "conflict");
    s.apply_classification_import(doc, vec!["application:code.exe".into()], after.token)
        .unwrap();
    assert_eq!(workspace(&s, "application").items[0].category_name, "学习");
}
#[test]
fn import_rejects_duplicate_unknown_and_malformed_targets() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    app(&s, "Editor", "code.exe", now - 1000, now);
    let (_, doc) = export_document(
        &s,
        &["application:code.exe"],
        serde_json::json!([{"key":"application:code.exe","category":"学习"},{"key":"application:code.exe","category":"工作"},{"key":"application:fake.exe","category":"学习"}]),
    );
    let p = s.preview_classification_import(doc.clone()).unwrap();
    assert!(p.rows.iter().all(|r| !r.selectable));
    assert!(s
        .apply_classification_import(doc, vec!["application:code.exe".into()], p.token)
        .is_err());
    assert!(s.preview_classification_import("not json".into()).is_err());
    assert!(s
        .preview_classification_import("x".repeat(2_000_001))
        .is_err());
}
#[test]
fn full_backup_preserves_category_and_import_undo_but_rejects_damaged_v6() {
    let source = store();
    let now = Utc::now().timestamp_millis();
    app(&source, "Editor", "code.exe", now - 1000, now);
    let (_, doc) = export_document(
        &source,
        &["application:code.exe"],
        serde_json::json!([{"key":"application:code.exe","category":"学习"}]),
    );
    let p = source.preview_classification_import(doc.clone()).unwrap();
    source
        .apply_classification_import(doc, vec!["application:code.exe".into()], p.token)
        .unwrap();
    let backup = source.path.with_extension("backup.sqlite");
    source.backup_to(backup.clone()).unwrap();
    let dest = store();
    dest.restore_from(backup).unwrap();
    assert!(workspace(&dest, "application").undo_available);
    assert_eq!(dest.undo_classification_import().unwrap().restored, 1);
    source
        .connection()
        .unwrap()
        .execute("UPDATE classification_undo SET payload='broken'", [])
        .unwrap();
    assert!(dest.restore_from(source.path.clone()).is_err());
    assert_eq!(workspace(&dest, "application").unclassified_count, 1);
}
#[test]
fn old_backup_migrates_existing_category_names() {
    let source = store();
    let now = Utc::now().timestamp_millis();
    app(&source, "Editor", "code.exe", now - 1000, now);
    source
        .set_default_category("application", "code.exe", "既有类别")
        .unwrap();
    let c = source.connection().unwrap();
    for table in [
        "classification_rules",
        "statistics_defaults",
        "statistics_overrides",
    ] {
        for suffix in ["insert", "update"] {
            c.execute_batch(&format!("DROP TRIGGER category_{table}_{suffix}"))
                .unwrap();
        }
    }
    c.execute_batch("DROP TABLE stat_categories;DROP TABLE classification_exports;DROP TABLE classification_undo;PRAGMA user_version=5;").unwrap();
    c.execute("INSERT INTO statistics_defaults(target_kind,target,category,updated_at) VALUES('application','legacy.exe','work',1),('application','another.exe','Work',2)",[]).unwrap();
    drop(c);
    let target = store();
    target.restore_from(source.path.clone()).unwrap();
    assert_eq!(workspace(&target, "application").categories.len(), 2);
    assert_eq!(
        workspace(&target, "application").items[0].category_name,
        "既有类别"
    );
    let connection = target.connection().unwrap();
    let variants:i64=connection.query_row("SELECT COUNT(DISTINCT category) FROM statistics_defaults WHERE target IN ('legacy.exe','another.exe')",[],|r|r.get(0)).unwrap();
    assert_eq!(variants, 1);
    let revisions:i64=connection.query_row("SELECT SUM(updated_at) FROM statistics_defaults WHERE target IN ('legacy.exe','another.exe')",[],|r|r.get(0)).unwrap();
    assert_eq!(revisions, 3);
}

#[test]
fn import_uses_canonical_new_names_and_binds_preview_to_content() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    app(&s, "Editor", "code.exe", now - 1000, now);
    let (_, doc) = export_document(
        &s,
        &["application:code.exe"],
        serde_json::json!([{"key":"application:code.exe","category":"学习"}]),
    );
    let original = s.preview_classification_import(doc.clone()).unwrap();
    let mut changed: serde_json::Value = serde_json::from_str(&doc).unwrap();
    changed["categories"] = serde_json::json!([{"name":"Study","color":"#32776f"}]);
    changed["assignments"][0]["category"] = "study".into();
    let changed = changed.to_string();
    assert!(s
        .apply_classification_import(
            changed.clone(),
            vec!["application:code.exe".into()],
            original.token
        )
        .is_err());
    let preview = s.preview_classification_import(changed.clone()).unwrap();
    assert_eq!(preview.rows[0].proposed_category, "Study");
    s.apply_classification_import(changed, vec!["application:code.exe".into()], preview.token)
        .unwrap();
    assert_eq!(workspace(&s, "application").items[0].category_name, "Study");
    // Explicitly assigning the same value afterward is still a user's later edit.
    s.set_default_category("application", "code.exe", "Study")
        .unwrap();
    assert_eq!(s.undo_classification_import().unwrap().skipped, 1);
}

#[test]
fn ipv6_website_import_can_be_undone_and_backed_up() {
    let s = store();
    let now = Utc::now().timestamp_millis();
    s.connection().unwrap().execute("INSERT INTO web_activity_segments(started_at,ended_at,domain,title,idle,browser_target) VALUES(?1,?2,'[::1]','',0,'msedge.exe')",params![now-1000,now]).unwrap();
    let sites = workspace(&s, "website");
    let key = sites.items[0].key.clone();
    let (_, doc) = export_document(
        &s,
        &[&key],
        serde_json::json!([{"key":key,"category":"学习"}]),
    );
    let preview = s.preview_classification_import(doc.clone()).unwrap();
    s.apply_classification_import(doc, vec![key], preview.token)
        .unwrap();
    let backup = s.path.with_extension("backup.sqlite");
    s.backup_to(backup.clone()).unwrap();
    let restored = store();
    restored.restore_from(backup).unwrap();
    assert_eq!(restored.undo_classification_import().unwrap().restored, 1);
}
