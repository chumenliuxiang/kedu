use super::*;
use crate::category_models::NewStatCategory;

struct Fixture(Storage);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join("kedu-tests").join(format!(
            "timeline-{}-{}.sqlite",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        Self(Storage::new(path).unwrap())
    }
    fn app(&self, start: i64, end: i64, name: &str, idle: bool) -> i64 {
        let c = self.0.connection().unwrap();
        c.execute("INSERT INTO activity_segments(started_at,ended_at,app_name,process_path,title,idle) VALUES(?1,?2,?3,?4,'title',?5)",params![start,end,name,format!("C:\\{name}.exe"),idle as i64]).unwrap();
        c.last_insert_rowid()
    }
    fn category(&self, name: &str) -> i64 {
        self.0
            .save_stat_category(NewStatCategory {
                id: None,
                name: name.into(),
                color: "#32776f".into(),
            })
            .unwrap()
            .id
    }
    fn categories(&self) -> Vec<(i64, i64, String)> {
        self.0
            .timeline_workspace(0, 100_000)
            .unwrap()
            .items
            .into_iter()
            .map(|i| (i.started_at, i.ended_at, i.category))
            .collect()
    }
    fn correct(
        &self,
        ranges: &[(i64, i64)],
        category: Option<i64>,
        only_unclassified: bool,
    ) -> AnnotationResult {
        self.0
            .annotate_timeline(
                ranges
                    .iter()
                    .map(|&(start, end)| TimeRange { start, end })
                    .collect(),
                category,
                only_unclassified,
            )
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", ".backup"] {
            let _ = fs::remove_file(format!("{}{suffix}", self.0.path.to_string_lossy()));
        }
    }
}

#[test]
fn timeline_partial_correction_preserves_raw_rows_and_all_statistics_totals() {
    let f = Fixture::new();
    let id = f.app(1_000, 61_000, "edge", false);
    let study = f.category("学习");
    f.0.set_default_category("application", "edge.exe", "工具")
        .unwrap();
    assert_eq!(
        f.correct(&[(21_000, 41_000)], Some(study), false)
            .changed_milliseconds,
        20_000
    );
    assert_eq!(
        f.categories(),
        vec![
            (1_000, 21_000, "工具".into()),
            (21_000, 41_000, "学习".into()),
            (41_000, 61_000, "工具".into())
        ]
    );
    let stats = f.0.query_statistics(query(0, 100_000)).unwrap();
    assert_eq!(stats.effective_milliseconds, 60_000);
    assert_eq!(
        stats.app_usage.iter().map(|r| r.milliseconds).sum::<i64>(),
        60_000
    );
    assert_eq!(
        stats
            .category_usage
            .iter()
            .map(|r| r.milliseconds)
            .sum::<i64>(),
        60_000
    );
    assert_eq!(
        stats.trend.iter().map(|r| r.milliseconds).sum::<i64>(),
        60_000
    );
    assert_eq!(
        f.0.connection()
            .unwrap()
            .query_row(
                "SELECT id,started_at,ended_at,app_name,title FROM activity_segments",
                [],
                |r| Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?
                ))
            )
            .unwrap(),
        (id, 1_000, 61_000, "edge".into(), "title".into())
    );
}

#[test]
fn timeline_overlapping_selections_are_unioned_and_idle_gaps_exclusions_untouched() {
    let f = Fixture::new();
    f.app(1_000, 11_000, "code", false);
    f.app(11_000, 21_000, "code", true);
    f.app(25_000, 35_000, "excluded", false);
    f.app(40_000, 50_000, "code", false);
    f.0.set_statistics_excluded("application", "excluded.exe", true)
        .unwrap();
    let study = f.category("学习");
    assert_eq!(
        f.correct(
            &[(0, 45_000), (5_000, 30_000), (44_000, 47_000)],
            Some(study),
            false
        )
        .changed_milliseconds,
        17_000
    );
    assert_eq!(
        f.categories(),
        vec![
            (1_000, 11_000, "学习".into()),
            (11_000, 21_000, "未分类".into()),
            (40_000, 47_000, "学习".into()),
            (47_000, 50_000, "未分类".into())
        ]
    );
    assert_eq!(
        f.0.query_statistics(query(0, 100_000))
            .unwrap()
            .effective_milliseconds,
        20_000
    );
    f.0.set_statistics_excluded("application", "excluded.exe", false)
        .unwrap();
    assert_eq!(f.categories()[2], (25_000, 35_000, "未分类".into()));
}

#[test]
fn timeline_only_unclassified_checks_effective_category_and_noop_keeps_undo() {
    let f = Fixture::new();
    f.app(1_000, 61_000, "edge", false);
    let study = f.category("学习");
    let work = f.category("工作");
    f.correct(&[(21_000, 41_000)], Some(study), false);
    assert_eq!(
        f.correct(&[(1_000, 61_000)], Some(work), true)
            .changed_milliseconds,
        40_000
    );
    assert_eq!(
        f.correct(&[(1_000, 61_000)], Some(work), true)
            .changed_milliseconds,
        0
    );
    assert_eq!(
        f.0.undo_timeline_annotation()
            .unwrap()
            .restored_milliseconds,
        40_000
    );
    assert_eq!(
        f.categories(),
        vec![
            (1_000, 21_000, "未分类".into()),
            (21_000, 41_000, "学习".into()),
            (41_000, 61_000, "未分类".into())
        ]
    );
    assert!(!f.0.timeline_workspace(0, 100_000).unwrap().undo_available);
}

#[test]
fn timeline_restore_auto_masks_record_override_and_undo_restores_previous_manual() {
    let f = Fixture::new();
    let app = f.app(1_000, 61_000, "code", false);
    let study = f.category("学习");
    f.0.set_default_category("application", "code.exe", "工具")
        .unwrap();
    f.0.set_activity_category("application", app, Some("工作"))
        .unwrap();
    f.correct(&[(11_000, 51_000)], Some(study), false);
    assert_eq!(
        f.correct(&[(21_000, 41_000)], None, false)
            .changed_milliseconds,
        20_000
    );
    assert_eq!(
        f.categories(),
        vec![
            (1_000, 11_000, "工作".into()),
            (11_000, 21_000, "学习".into()),
            (21_000, 41_000, "工具".into()),
            (41_000, 51_000, "学习".into()),
            (51_000, 61_000, "工作".into())
        ]
    );
    assert_eq!(
        f.0.undo_timeline_annotation()
            .unwrap()
            .restored_milliseconds,
        20_000
    );
    assert_eq!(f.categories()[1], (11_000, 51_000, "学习".into()));
}

#[test]
fn timeline_undo_after_restart_preserves_subsequent_record_correction() {
    let f = Fixture::new();
    let app = f.app(1_000, 21_000, "code", false);
    f.app(21_000, 41_000, "edge", false);
    let study = f.category("学习");
    f.correct(&[(1_000, 41_000)], Some(study), false);
    f.0.set_activity_category("application", app, Some("工作"))
        .unwrap();
    let reopened = Storage::new(f.0.path.clone()).unwrap();
    assert_eq!(
        reopened
            .undo_timeline_annotation()
            .unwrap()
            .restored_milliseconds,
        20_000
    );
    assert_eq!(
        f.categories(),
        vec![
            (1_000, 21_000, "工作".into()),
            (21_000, 41_000, "未分类".into())
        ]
    );
    assert_eq!(
        reopened
            .undo_timeline_annotation()
            .unwrap()
            .restored_milliseconds,
        0
    );
}

#[test]
fn timeline_explicit_unclassified_is_manual_and_category_rename_merge_work() {
    let f = Fixture::new();
    f.app(1_000, 61_000, "code", false);
    let study = f.category("学习");
    let work = f.category("工作");
    f.0.set_default_category("application", "code.exe", "工具")
        .unwrap();
    f.correct(&[(1_000, 21_000)], Some(0), false);
    f.correct(&[(21_000, 41_000)], Some(study), false);
    f.0.save_stat_category(NewStatCategory {
        id: Some(study),
        name: "课程".into(),
        color: "#32776f".into(),
    })
    .unwrap();
    assert_eq!(f.categories()[1].2, "课程");
    f.0.remove_stat_category(study, Some(work)).unwrap();
    assert_eq!(f.categories()[1].2, "工作");
    f.0.remove_stat_category(work, None).unwrap();
    assert_eq!(f.categories()[0], (1_000, 41_000, "未分类".into()));
    f.correct(&[(1_000, 41_000)], None, false);
    assert_eq!(f.categories(), vec![(1_000, 61_000, "工具".into())]);
    validate(&f.0.connection().unwrap()).unwrap();
}

#[test]
fn timeline_backup_restores_overlay_and_undo_and_legacy_version_seven_migrates() {
    let f = Fixture::new();
    f.app(1_000, 61_000, "code", false);
    let study = f.category("学习");
    f.correct(&[(21_000, 41_000)], Some(study), false);
    let backup = PathBuf::from(format!("{}.backup", f.0.path.to_string_lossy()));
    f.0.backup_to(backup.clone()).unwrap();
    let target = Fixture::new();
    target.0.restore_from(backup).unwrap();
    assert_eq!(target.categories(), f.categories());
    assert_eq!(
        target
            .0
            .undo_timeline_annotation()
            .unwrap()
            .restored_milliseconds,
        20_000
    );
    let old = Fixture::new();
    let app = old.app(1_000, 21_000, "edge", false);
    old.0
        .set_activity_category("application", app, Some("历史分类"))
        .unwrap();
    old.0.connection().unwrap().execute_batch("DROP TABLE timeline_annotation_undo;DROP TABLE timeline_annotations;DROP TABLE timeline_annotation_batches;PRAGMA user_version=7;").unwrap();
    target.0.restore_from(old.0.path.clone()).unwrap();
    assert_eq!(
        target.categories(),
        vec![(1_000, 21_000, "历史分类".into())]
    );
    assert_eq!(
        target
            .0
            .connection()
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        9
    );
    assert!(
        !target
            .0
            .timeline_workspace(0, 100_000)
            .unwrap()
            .undo_available
    );
}

#[test]
fn timeline_malformed_backup_is_rejected_before_live_database_changes() {
    let live = Fixture::new();
    live.app(1_000, 21_000, "code", false);
    for corrupted in 0..3 {
        let bad = Fixture::new();
        bad.app(1_000, 61_000, "edge", false);
        let study = bad.category("学习");
        bad.correct(&[(21_000, 41_000)], Some(study), false);
        let c = bad.0.connection().unwrap();
        match corrupted {
            0 => {
                c.execute_batch("DROP TABLE timeline_annotation_undo;")
                    .unwrap();
            }
            1 => {
                c.execute(
                    "UPDATE timeline_annotations SET category='missing category'",
                    [],
                )
                .unwrap();
            }
            _ => {
                c.execute_batch(
                    "PRAGMA foreign_keys=OFF;UPDATE timeline_annotations SET batch_id=99999;",
                )
                .unwrap();
            }
        }
        drop(c);
        assert!(live.0.restore_from(bad.0.path.clone()).is_err());
        assert_eq!(live.categories(), vec![(1_000, 21_000, "未分类".into())]);
    }
}

#[test]
fn timeline_invalid_request_is_atomic_and_many_records_are_not_truncated() {
    let f = Fixture::new();
    let study = f.category("学习");
    {
        let mut c = f.0.connection().unwrap();
        let tx = c.transaction().unwrap();
        for n in 0..700 {
            tx.execute("INSERT INTO activity_segments(started_at,ended_at,app_name,process_path,title,idle) VALUES(?1,?2,'code','C:\\code.exe',?3,0)",params![n*100,n*100+90,format!("title{n}")]).unwrap();
        }
        tx.commit().unwrap();
    }
    assert_eq!(f.0.timeline_workspace(0, 100_000).unwrap().items.len(), 700);
    assert_eq!(
        f.correct(&[(0, 100_000)], Some(study), false)
            .changed_milliseconds,
        63_000
    );
    let before = f.categories();
    assert!(f
        .0
        .annotate_timeline(vec![TimeRange { start: 10, end: 0 }], Some(study), false)
        .is_err());
    assert!(f
        .0
        .annotate_timeline(vec![TimeRange { start: 0, end: 100 }], Some(999999), false)
        .is_err());
    assert!(f
        .0
        .annotate_timeline(
            vec![TimeRange {
                start: 0,
                end: i64::MAX
            }],
            Some(study),
            false
        )
        .is_err());
    assert_eq!(f.categories(), before);
    assert_eq!(
        f.0.undo_timeline_annotation()
            .unwrap()
            .restored_milliseconds,
        63_000
    );
}

#[test]
fn timeline_explicit_assignment_pins_a_matching_automatic_category() {
    let f = Fixture::new();
    f.app(1_000, 61_000, "code", false);
    let study = f.category("学习");
    f.0.set_default_category("application", "code.exe", "学习")
        .unwrap();
    assert_eq!(
        f.correct(&[(21_000, 41_000)], Some(study), false)
            .changed_milliseconds,
        20_000
    );
    f.0.set_default_category("application", "code.exe", "工作")
        .unwrap();
    assert_eq!(
        f.categories(),
        vec![
            (1_000, 21_000, "工作".into()),
            (21_000, 41_000, "学习".into()),
            (41_000, 61_000, "工作".into())
        ]
    );
    f.correct(&[(21_000, 41_000)], None, false);
    assert_eq!(f.categories(), vec![(1_000, 61_000, "工作".into())]);
}

#[test]
fn timeline_website_and_application_views_share_partial_corrections() {
    let f = Fixture::new();
    f.app(1_000, 61_000, "msedge", false);
    let study = f.category("学习");
    f.0.connection().unwrap().execute("INSERT INTO web_activity_segments(started_at,ended_at,domain,title,idle,browser_target) VALUES(11000,51000,'www.bilibili.com','course',0,'msedge.exe')",[]).unwrap();
    f.0.set_default_category("website", "bilibili.com", "娱乐")
        .unwrap();
    f.correct(&[(21_000, 41_000)], Some(study), false);
    let mut q = query(0, 100_000);
    q.target_kind = Some("website".into());
    q.target = Some("bilibili.com".into());
    let stats = f.0.query_statistics(q).unwrap();
    assert_eq!(stats.effective_milliseconds, 40_000);
    assert_eq!(
        stats
            .category_usage
            .iter()
            .find(|r| r.app_name == "学习")
            .unwrap()
            .milliseconds,
        20_000
    );
    let all = f.0.query_statistics(query(0, 100_000)).unwrap();
    assert_eq!(all.effective_milliseconds, 60_000);
    assert_eq!(
        all.category_usage
            .iter()
            .find(|r| r.app_name == "学习")
            .unwrap()
            .milliseconds,
        20_000
    );
    assert_eq!(
        all.site_usage.iter().map(|r| r.milliseconds).sum::<i64>(),
        40_000
    );
}
