//! Persist successful local icons independently of the lifetime of an EXE/window.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::io::Cursor;

const REFRESH_MS: i64 = 30 * 86_400_000;
const MAX_ENTRIES: i64 = 2048;
const PNG_PREFIX: &str = "data:image/png;base64,";

pub(super) fn migrate(conn: &Connection, version: i64) -> Result<(), String> {
    if version >= 9 {
        validate(conn)?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS application_icon_cache (
        application_key TEXT PRIMARY KEY, png_data_url TEXT NOT NULL,
        source_path TEXT NOT NULL, updated_at INTEGER NOT NULL);",
    )
    .map_err(|error| error.to_string())
}

pub(super) fn validate(conn: &Connection) -> Result<(), String> {
    conn.prepare(
        "SELECT application_key,png_data_url,source_path,updated_at FROM application_icon_cache",
    )
    .map_err(|error| format!("备份缺少应用图标缓存：{error}"))?;
    Ok(())
}

fn valid_png(value: &str) -> bool {
    if value.len() > 128 * 1024 {
        return false;
    }
    let Some(encoded) = value.strip_prefix(PNG_PREFIX) else {
        return false;
    };
    let Ok(bytes) = STANDARD.decode(encoded) else {
        return false;
    };
    let reader = image::ImageReader::with_format(Cursor::new(&bytes), image::ImageFormat::Png);
    let Ok((width, height)) = reader.into_dimensions() else {
        return false;
    };
    (1..=256).contains(&width)
        && (1..=256).contains(&height)
        && image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).is_ok()
}

impl Storage {
    fn cached_application_icon(&self, key: &str) -> Result<Option<(String, i64)>, String> {
        let row: Option<(String, i64)> = self.connection()?.query_row(
            "SELECT png_data_url,updated_at FROM application_icon_cache WHERE application_key=?1",
            [key], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(|error| error.to_string())?;
        Ok(row.filter(|(image, _)| valid_png(image)))
    }

    pub fn remember_application_icon(&self, path: &str, image: &str) -> Result<bool, String> {
        if !valid_png(image) {
            return Err("无效的应用图标".into());
        }
        let key = crate::app_identity::canonical_executable(path);
        let changed = self
            .cached_application_icon(&key)?
            .is_none_or(|(old, _)| old != image);
        let mut conn = self.connection()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO application_icon_cache(application_key,png_data_url,source_path,updated_at) VALUES(?1,?2,?3,?4)
            ON CONFLICT(application_key) DO UPDATE SET png_data_url=excluded.png_data_url,source_path=excluded.source_path,updated_at=excluded.updated_at",
            params![key,image,path,Utc::now().timestamp_millis()]).map_err(|e|e.to_string())?;
        tx.execute("DELETE FROM application_icon_cache WHERE application_key IN (
            SELECT application_key FROM application_icon_cache ORDER BY updated_at DESC,application_key LIMIT -1 OFFSET ?1)",
            [MAX_ENTRIES]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(changed)
    }

    fn application_icon_paths(&self, target: &str) -> Result<Vec<String>, String> {
        let mut paths = Vec::new();
        if Path::new(target).is_file() {
            paths.push(target.to_string());
        }
        let key = crate::app_identity::canonical_executable(target);
        let names: &[&str] = if key == "steam.exe" {
            &["steam.exe", "steamwebhelper.exe"]
        } else {
            &[key.as_str()]
        };
        let conn = self.connection()?;
        for name in names {
            let suffix = format!("%\\{name}");
            let mut query = conn.prepare("SELECT process_path,MAX(ended_at) FROM activity_segments
                WHERE process_path<>'' AND (lower(replace(process_path,'/','\\')) LIKE ?1 ESCAPE '|' OR lower(process_path)=?2 OR lower(app_name)=?3)
                GROUP BY lower(process_path) ORDER BY MAX(ended_at) DESC LIMIT 32").map_err(|e|e.to_string())?;
            // Escape SQL pattern characters in an executable name.
            let suffix = suffix
                .chars()
                .enumerate()
                .fold(String::new(), |mut s, (i, c)| {
                    if i != 0 && matches!(c, '%' | '_' | '|') {
                        s.push('|');
                    }
                    s.push(c);
                    s
                });
            let rows = query
                .query_map(
                    params![suffix, name, normalize_application_subject(target)],
                    |r| r.get::<_, String>(0),
                )
                .map_err(|e| e.to_string())?;
            for path in rows {
                let path = path.map_err(|e| e.to_string())?;
                if Path::new(&path).is_file()
                    && !paths.iter().any(|p| p.eq_ignore_ascii_case(&path))
                {
                    paths.push(path);
                }
            }
        }
        Ok(paths)
    }

    /// Called from a blocking worker. A refresh miss never erases the last good image.
    pub fn application_icon(&self, target: &str) -> Result<Option<String>, String> {
        let key = crate::app_identity::canonical_executable(target);
        let cached = self.cached_application_icon(&key)?;
        if let Some((image, at)) = &cached {
            if Utc::now().timestamp_millis().saturating_sub(*at) < REFRESH_MS {
                return Ok(Some(image.clone()));
            }
        }
        for path in self.application_icon_paths(target)? {
            if let Some(image) = crate::platform::application_icon_data_url(&path) {
                self.remember_application_icon(&path, &image)?;
                return Ok(Some(image));
            }
        }
        Ok(cached.map(|(image, _)| image))
    }

    /// Capture while the app is running, even when Kedu's WebView is closed.
    pub fn capture_application_icon(&self, path: &str) -> Result<bool, String> {
        let key = crate::app_identity::canonical_executable(path);
        if self
            .cached_application_icon(&key)?
            .is_some_and(|(_, at)| Utc::now().timestamp_millis().saturating_sub(at) < REFRESH_MS)
        {
            return Ok(false);
        }
        let image = crate::platform::application_icon_data_url(path)
            .or_else(|| crate::platform::foreground_application_icon(path));
        match image {
            Some(image) => self.remember_application_icon(path, &image),
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png() -> String {
        let mut out = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(2, 2, image::Rgba([25, 120, 90, 255]))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        format!("{PNG_PREFIX}{}", STANDARD.encode(out.into_inner()))
    }
    #[test]
    fn saved_icons_survive_missing_executable_restart_and_failed_refresh() {
        let dir = std::env::temp_dir().join(format!(
            "kedu-icons-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let db = dir.join("kedu.sqlite");
        let storage = Storage::new(db.clone()).unwrap();
        let image = png();
        assert!(storage
            .remember_application_icon(r"Z:\missing\steamwebhelper.exe", &image)
            .unwrap());
        drop(storage);
        let storage = Storage::new(db).unwrap();
        assert_eq!(
            storage.application_icon("steam.exe").unwrap(),
            Some(image.clone())
        );
        storage
            .connection()
            .unwrap()
            .execute("UPDATE application_icon_cache SET updated_at=0", [])
            .unwrap();
        assert_eq!(
            storage.application_icon("steamwebhelper.exe").unwrap(),
            Some(image)
        );
        assert_eq!(
            storage.application_icon("never-recorded.exe").unwrap(),
            None
        );
        assert!(storage
            .remember_application_icon("x.exe", "data:image/svg+xml,bad")
            .is_err());
        let backup = dir.join("backup.sqlite");
        storage.backup_to(backup.clone()).unwrap();
        let restored = Storage::new(dir.join("restored.sqlite")).unwrap();
        restored.restore_from(backup).unwrap();
        assert!(restored.application_icon("steam.exe").unwrap().is_some());
        let legacy = dir.join("legacy.sqlite");
        storage.backup_to(legacy.clone()).unwrap();
        let old = Connection::open(&legacy).unwrap();
        old.execute_batch("DROP TABLE application_icon_cache; PRAGMA user_version=8;")
            .unwrap();
        drop(old);
        restored.restore_from(legacy).unwrap();
        assert_eq!(restored.application_icon("steam.exe").unwrap(), None);
        drop(storage);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn steam_statistics_categories_quotas_whitelist_and_mode_share_persisted_identity() {
        use crate::domain::{FocusPreferences, FocusWhitelistInput};
        let dir = std::env::temp_dir().join("kedu-tests").join(format!(
            "steam-{}",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let storage = Storage::new(dir.join("kedu.sqlite")).unwrap();
        let now = Utc::now().timestamp_millis();
        let start = now - 180_000;
        let db = storage.connection().unwrap();
        for (offset, name, path) in [
            (0, "steam", r"F:\Steam\steam.exe"),
            (
                60_000,
                "steamwebhelper",
                r"F:\Steam\bin\cef\cef.win64\steamwebhelper.exe",
            ),
            (120_000, "Game", r"F:\Steam\steamapps\common\Game\Game.exe"),
        ] {
            db.execute("INSERT INTO activity_segments(started_at,ended_at,app_name,process_path,title,idle) VALUES(?1,?2,?3,?4,'test',0)",params![start+offset,start+offset+60_000,name,path]).unwrap();
        }
        db.execute_batch("INSERT INTO limit_rules(name,target_kind,target_pattern,group_name,daily_limit_minutes,enabled) VALUES('Steam','application','steam.exe','client',1,1),('Helper','application','steamwebhelper.exe','client',1,1),('Group','group','client','',1,1);").unwrap();
        storage
            .set_default_category("application", "steamwebhelper.exe", "娱乐")
            .unwrap();
        let stats = storage.statistics(start, now).unwrap();
        assert_eq!(stats.effective_seconds, 180);
        assert_eq!(
            stats
                .app_usage
                .iter()
                .find(|r| r.app_name == "Steam")
                .unwrap()
                .seconds,
            120
        );
        assert_eq!(
            stats
                .category_usage
                .iter()
                .find(|r| r.app_name == "娱乐")
                .unwrap()
                .seconds,
            120
        );
        for rule in storage.list_rules().unwrap() {
            assert_eq!(rule.used_seconds, 120);
        }
        storage
            .save_focus_whitelist(FocusWhitelistInput {
                id: None,
                target_kind: "application".into(),
                target: "steam.exe".into(),
                display_name: "Steam".into(),
                path: None,
            })
            .unwrap();
        assert!(storage
            .focus_allowed("application", "steamwebhelper.exe", now + 1000)
            .unwrap());
        assert!(!storage
            .focus_allowed("application", r"F:\Steam\steamapps\Game.exe", now + 1000)
            .unwrap());
        storage
            .set_statistics_excluded("application", "steamwebhelper.exe", true)
            .unwrap();
        assert_eq!(
            storage.statistics(start, now).unwrap().effective_seconds,
            60
        );
        storage
            .set_statistics_excluded("application", "steam.exe", false)
            .unwrap();
        assert_eq!(
            storage.statistics(start, now).unwrap().effective_seconds,
            180
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM activity_segments WHERE app_name='steamwebhelper'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            storage.focus_preferences().unwrap().window_guard_mode,
            "reminder"
        );
        let prefs = FocusPreferences {
            window_guard_mode: "minimize".into(),
            ..FocusPreferences::default()
        };
        storage.save_focus_preferences(prefs.clone()).unwrap();
        let reopened = Storage::new(dir.join("kedu.sqlite")).unwrap();
        assert_eq!(reopened.focus_preferences().unwrap(), prefs);
        let mut invalid = prefs;
        invalid.window_guard_mode = "invalid".into();
        assert!(reopened.save_focus_preferences(invalid).is_err());
        drop(db);
        drop(storage);
        drop(reopened);
        let _ = fs::remove_dir_all(dir);
    }
}
