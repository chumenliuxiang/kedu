//! Per-rule reminder thresholds and durable once-per-statistical-day delivery.
use super::*;
use rusqlite::{Transaction, TransactionBehavior};

pub(super) fn migrate(tx: &Transaction<'_>) -> Result<(), String> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS limit_reminder_settings (
           rule_id INTEGER PRIMARY KEY REFERENCES limit_rules(id) ON DELETE CASCADE,
           minutes_json TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS limit_reminder_events (
           rule_id INTEGER NOT NULL REFERENCES limit_rules(id) ON DELETE CASCADE,
           day_started_at INTEGER NOT NULL, threshold_minutes INTEGER NOT NULL,
           occurred_at INTEGER NOT NULL,
           PRIMARY KEY(rule_id,day_started_at,threshold_minutes));
         CREATE TABLE IF NOT EXISTS limit_reminder_state (
           rule_id INTEGER PRIMARY KEY REFERENCES limit_rules(id) ON DELETE CASCADE,
           day_started_at INTEGER NOT NULL, remaining_seconds INTEGER NOT NULL,
           observed_at INTEGER NOT NULL, limit_seconds INTEGER NOT NULL);",
    )
    .map_err(|error| error.to_string())
}

pub struct LimitReminder {
    pub rule_name: String,
    pub remaining_seconds: i64,
}

pub(super) fn validate(connection: &Connection) -> Result<(), String> {
    let mut statement = connection
        .prepare(
            "SELECT s.minutes_json, r.daily_limit_minutes FROM limit_reminder_settings s
         LEFT JOIN limit_rules r ON r.id=s.rule_id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (json, limit) = row.map_err(|error| error.to_string())?;
        let limit = limit.ok_or("备份中的提醒关联了不存在的限制规则")?;
        let minutes: Vec<i64> =
            serde_json::from_str(&json).map_err(|_| "备份中的提醒配置无法读取".to_string())?;
        // Older clients may lower the quota while retaining a larger threshold.
        // That threshold remains harmless and is filtered at delivery time.
        if limit < 1
            || minutes.len() > 32
            || minutes.iter().any(|value| !(1..=1440).contains(value))
        {
            return Err("备份中的提醒时间无效".into());
        }
    }
    connection.prepare("SELECT rule_id,day_started_at,threshold_minutes,occurred_at FROM limit_reminder_events")
        .map_err(|error| error.to_string())?;
    connection.prepare("SELECT rule_id,day_started_at,remaining_seconds,observed_at,limit_seconds FROM limit_reminder_state")
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn validate_minutes(minutes: &[i64], limit: i64) -> Result<Vec<i64>, String> {
    if minutes.len() > 32 {
        return Err("最多设置 32 个提醒时间点".into());
    }
    if minutes.iter().any(|value| *value < 1 || *value >= limit) {
        return Err("提醒时间须为大于 0 且小于每日额度的整数分钟".into());
    }
    let mut values = minutes.to_vec();
    values.sort_unstable_by(|a, b| b.cmp(a));
    values.dedup();
    Ok(values)
}

fn configured_minutes(connection: &Connection, id: i64, limit: i64) -> Result<Vec<i64>, String> {
    let configured: Option<String> = connection
        .query_row(
            "SELECT minutes_json FROM limit_reminder_settings WHERE rule_id=?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match configured {
        Some(json) => {
            let mut values: Vec<i64> = serde_json::from_str(&json)
                .map_err(|_| "提醒配置无法读取，请重新保存".to_string())?;
            // Older API clients may lower a rule's quota without editing reminders.
            values.retain(|value| *value > 0 && *value < limit);
            values.sort_unstable_by(|a, b| b.cmp(a));
            values.dedup();
            Ok(values)
        }
        None => Ok([5, 1].into_iter().filter(|value| *value < limit).collect()),
    }
}

fn persist_minutes(tx: &Transaction<'_>, id: i64, minutes: &[i64]) -> Result<(), String> {
    let json = serde_json::to_string(minutes).map_err(|error| error.to_string())?;
    tx.execute(
        "INSERT INTO limit_reminder_settings(rule_id,minutes_json) VALUES(?1,?2)
         ON CONFLICT(rule_id) DO UPDATE SET minutes_json=excluded.minutes_json",
        params![id, json],
    )
    .map_err(|error| error.to_string())?;
    // Delivery history deliberately survives editing: the same threshold cannot
    // ring again today just because a dialog was saved or the app restarted.
    tx.execute(
        "DELETE FROM limit_reminder_state WHERE rule_id=?1",
        params![id],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

impl Storage {
    pub fn get_limit_reminders(&self, rule_id: i64) -> Result<Vec<i64>, String> {
        let connection = self.connection()?;
        let limit: i64 = connection
            .query_row(
                "SELECT daily_limit_minutes FROM limit_rules WHERE id=?1",
                params![rule_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or("限制规则不存在")?;
        configured_minutes(&connection, rule_id, limit)
    }

    pub fn save_limit_reminders(
        &self,
        rule_id: i64,
        minutes: Vec<i64>,
    ) -> Result<Vec<i64>, String> {
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let limit: i64 = tx
            .query_row(
                "SELECT daily_limit_minutes FROM limit_rules WHERE id=?1",
                params![rule_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or("限制规则不存在")?;
        let minutes = validate_minutes(&minutes, limit)?;
        persist_minutes(&tx, rule_id, &minutes)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(minutes)
    }

    /// The rule and its reminder points must save together; a failed reminder
    /// validation must not leave an extra rule that the next click duplicates.
    pub fn save_rule_with_reminders(
        &self,
        id: Option<i64>,
        rule: NewLimitRule,
        minutes: Vec<i64>,
    ) -> Result<LimitRule, String> {
        if rule.name.trim().is_empty() || rule.target_pattern.trim().is_empty() {
            return Err("名称和匹配目标不能为空".into());
        }
        if !matches!(
            rule.target_kind.as_str(),
            "application" | "website" | "group"
        ) {
            return Err("未知的限制目标类型".into());
        }
        if !(1..=1440).contains(&rule.daily_limit_minutes) {
            return Err("每日额度必须在 1 到 1440 分钟之间".into());
        }
        let minutes = validate_minutes(&minutes, rule.daily_limit_minutes)?;
        // Reconcile any elapsed focus before entering the rule transaction.
        let _ = self.active_focus()?;
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let id = if let Some(id) = id {
            let enabled = tx
                .query_row(
                    "SELECT enabled FROM limit_rules WHERE id=?1",
                    params![id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|error| error.to_string())?
                .ok_or("限制规则不存在")?
                != 0;
            let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM focus_sessions WHERE phase IN ('focusing','away'))", [], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            if enabled && active {
                return Err("专注进行中，请先关闭这条规则并等待倒计时结束后再编辑".into());
            }
            tx.execute(
                "UPDATE limit_rules SET name=?1,target_kind=?2,target_pattern=?3,group_name=?4,daily_limit_minutes=?5 WHERE id=?6",
                params![rule.name.trim(),rule.target_kind,rule.target_pattern.trim(),rule.group_name.trim(),rule.daily_limit_minutes,id],
            ).map_err(|error| error.to_string())?;
            id
        } else {
            if rule.target_kind == "website" {
                return Err("本轮仅支持应用和组的限制，网站仅记录使用时长".into());
            }
            tx.execute(
                "INSERT INTO limit_rules(name,target_kind,target_pattern,group_name,daily_limit_minutes) VALUES(?1,?2,?3,?4,?5)",
                params![rule.name.trim(),rule.target_kind,rule.target_pattern.trim(),rule.group_name.trim(),rule.daily_limit_minutes],
            ).map_err(|error| error.to_string())?;
            tx.last_insert_rowid()
        };
        persist_minutes(&tx, id, &minutes)?;
        tx.commit().map_err(|error| error.to_string())?;
        self.list_rules()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| "无法读取已保存的限制".into())
    }

    pub fn take_limit_reminders(
        &self,
        activity: &ActivityIdentity,
    ) -> Result<Vec<LimitReminder>, String> {
        if activity.idle {
            return Ok(Vec::new());
        }
        let rules = self.list_rules()?;
        let relevant: Vec<_> = rules
            .iter()
            .filter(|rule| rule.enabled)
            .filter(|rule| {
                if rule.target_kind == "application" {
                    application_matches_rule(activity, rule)
                } else if rule.target_kind == "group" {
                    rules.iter().any(|member| {
                        member.enabled
                            && member.target_kind == "application"
                            && rule_belongs_to_group(member, rule)
                            && application_matches_rule(activity, member)
                    })
                } else {
                    false
                }
            })
            .cloned()
            .collect();
        self.take_limit_reminders_at(
            &relevant,
            self.today_start_millis(),
            Utc::now().timestamp_millis(),
        )
    }

    fn take_limit_reminders_at(
        &self,
        rules: &[LimitRule],
        day: i64,
        now: i64,
    ) -> Result<Vec<LimitReminder>, String> {
        if rules.is_empty() {
            return Ok(Vec::new());
        }
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let mut notifications = Vec::new();
        for rule in rules {
            let minutes = configured_minutes(&tx, rule.id, rule.daily_limit_minutes)?;
            let limit_seconds = rule.daily_limit_minutes * 60;
            let previous: Option<(i64, i64, i64)> = tx.query_row(
                "SELECT remaining_seconds,observed_at,limit_seconds FROM limit_reminder_state WHERE rule_id=?1 AND day_started_at=?2",
                params![rule.id,day], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            ).optional().map_err(|error| error.to_string())?;
            let (before, observed) = previous
                .filter(|item| item.2 == limit_seconds)
                .map(|item| (item.0, item.1))
                .unwrap_or((limit_seconds, i64::MIN));
            let remaining = rule.remaining_seconds;
            let mut should_notify = false;
            for threshold in minutes {
                let seconds = threshold * 60;
                if remaining > seconds {
                    continue;
                }
                let claimed = tx.execute(
                    "INSERT OR IGNORE INTO limit_reminder_events(rule_id,day_started_at,threshold_minutes,occurred_at) VALUES(?1,?2,?3,?4)",
                    params![rule.id,day,threshold,now],
                ).map_err(|error| error.to_string())? != 0;
                // A delayed/restarted collector consumes missed thresholds quietly.
                // Within the 5-second sampling window it still delivers the latest
                // threshold, even if this is the first sample for today's rule.
                let recent = now >= observed && now.saturating_sub(observed) <= 15_000;
                let near_threshold = remaining >= seconds - 10;
                if claimed && remaining > 0 && before > seconds && (recent || near_threshold) {
                    should_notify = true;
                }
            }
            if should_notify {
                notifications.push(LimitReminder {
                    rule_name: rule.name.clone(),
                    remaining_seconds: remaining,
                });
            }
            tx.execute(
                "INSERT INTO limit_reminder_state(rule_id,day_started_at,remaining_seconds,observed_at,limit_seconds) VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(rule_id) DO UPDATE SET day_started_at=excluded.day_started_at,remaining_seconds=excluded.remaining_seconds,observed_at=excluded.observed_at,limit_seconds=excluded.limit_seconds",
                params![rule.id,day,remaining,now,limit_seconds],
            ).map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(notifications)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn store() -> Storage {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join("kedu-tests").join(format!(
            "reminders-{}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(".kedu-test-data"), "").unwrap();
        Storage::new(directory.join("kedu.sqlite")).unwrap()
    }
    fn input() -> NewLimitRule {
        NewLimitRule {
            name: "测试额度".into(),
            target_kind: "application".into(),
            target_pattern: "sample.exe".into(),
            group_name: String::new(),
            daily_limit_minutes: 30,
        }
    }
    fn remaining(rule: &LimitRule, seconds: i64) -> LimitRule {
        let mut result = rule.clone();
        result.remaining_seconds = seconds;
        result.used_seconds = 1800 - seconds;
        result
    }

    #[test]
    fn custom_points_are_atomic_normalized_and_can_be_disabled() {
        let storage = store();
        assert!(storage
            .save_rule_with_reminders(None, input(), vec![30])
            .is_err());
        assert!(storage.list_rules().unwrap().is_empty());
        let rule = storage
            .save_rule_with_reminders(None, input(), vec![1, 10, 5, 10])
            .unwrap();
        assert_eq!(
            storage.get_limit_reminders(rule.id).unwrap(),
            vec![10, 5, 1]
        );
        let mut update = input();
        update.name = "不应保存".into();
        assert!(storage
            .save_rule_with_reminders(Some(rule.id), update, vec![0])
            .is_err());
        assert_eq!(storage.list_rules().unwrap()[0].name, "测试额度");
        storage.save_limit_reminders(rule.id, vec![]).unwrap();
        assert!(storage.get_limit_reminders(rule.id).unwrap().is_empty());
        assert!(storage
            .take_limit_reminders_at(&[remaining(&rule, 60)], 0, 1000)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn thresholds_fire_once_across_restart_and_reset_at_statistical_day() {
        let storage = store();
        let rule = storage
            .save_rule_with_reminders(None, input(), vec![10, 5, 1])
            .unwrap();
        assert!(storage
            .take_limit_reminders_at(&[remaining(&rule, 601)], 123, 1000)
            .unwrap()
            .is_empty());
        assert_eq!(
            storage
                .take_limit_reminders_at(&[remaining(&rule, 597)], 123, 6000)
                .unwrap()
                .len(),
            1
        );
        let reopened = Storage::new(storage.path.clone()).unwrap();
        assert!(reopened
            .take_limit_reminders_at(&[remaining(&rule, 590)], 123, 11000)
            .unwrap()
            .is_empty());
        assert_eq!(
            reopened
                .take_limit_reminders_at(&[remaining(&rule, 299)], 123, 16000)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            reopened
                .take_limit_reminders_at(&[remaining(&rule, 59)], 123, 21000)
                .unwrap()
                .len(),
            1
        );
        assert!(reopened
            .take_limit_reminders_at(&[remaining(&rule, 0)], 123, 26000)
            .unwrap()
            .is_empty());
        assert_eq!(
            reopened
                .take_limit_reminders_at(&[remaining(&rule, 599)], 456, 31000)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn missed_thresholds_do_not_burst_or_reappear_after_editing() {
        let storage = store();
        let rule = storage
            .save_rule_with_reminders(None, input(), vec![10, 5, 1])
            .unwrap();
        assert!(storage
            .take_limit_reminders_at(&[remaining(&rule, 601)], 0, 1000)
            .unwrap()
            .is_empty());
        assert!(storage
            .take_limit_reminders_at(&[remaining(&rule, 120)], 0, 300000)
            .unwrap()
            .is_empty());
        storage
            .save_limit_reminders(rule.id, vec![10, 5, 2, 1])
            .unwrap();
        assert!(
            storage
                .take_limit_reminders_at(&[remaining(&rule, 115)], 0, 305000)
                .unwrap()
                .len()
                <= 1
        );
        assert_eq!(
            storage
                .take_limit_reminders_at(&[remaining(&rule, 59)], 0, 310000)
                .unwrap()
                .len(),
            1
        );
        storage
            .save_limit_reminders(rule.id, vec![10, 5, 1])
            .unwrap();
        assert!(storage
            .take_limit_reminders_at(&[remaining(&rule, 55)], 0, 315000)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn reminder_configuration_roundtrips_full_backup_and_cascades_delete() {
        let storage = store();
        let rule = storage
            .save_rule_with_reminders(None, input(), vec![12, 4])
            .unwrap();
        let backup = storage.path.with_file_name("backup.sqlite");
        storage.backup_to(backup.clone()).unwrap();
        storage.save_limit_reminders(rule.id, vec![]).unwrap();
        storage.restore_from(backup).unwrap();
        assert_eq!(storage.get_limit_reminders(rule.id).unwrap(), vec![12, 4]);
        storage.delete_rule(rule.id).unwrap();
        let count: i64 = storage
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM limit_reminder_settings", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn invalid_reminder_backup_does_not_replace_user_records() {
        let storage = store();
        let rule = storage
            .save_rule_with_reminders(None, input(), vec![12, 4])
            .unwrap();
        let backup = storage.path.with_file_name("invalid-backup.sqlite");
        storage.backup_to(backup.clone()).unwrap();
        Connection::open(&backup)
            .unwrap()
            .execute(
                "UPDATE limit_reminder_settings SET minutes_json='broken'",
                [],
            )
            .unwrap();
        assert!(storage.restore_from(backup).is_err());
        assert_eq!(storage.get_limit_reminders(rule.id).unwrap(), vec![12, 4]);
    }
}
