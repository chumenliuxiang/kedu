//! UI appearance is stored independently from collector settings.
use super::Storage;
use rusqlite::OptionalExtension;

const DEFAULT_THEME: &str = "light";
const DEFAULT_ACCENT: &str = "blue";

/// `light`, `dark` and `system` are current. Earlier palette names stay valid so
/// restoring an older backup keeps the user's light/dark choice.
fn is_valid_theme(theme: &str) -> bool {
    matches!(
        theme,
        "light" | "dark" | "system" | "celadon" | "paper" | "mist" | "ink" | "midnight" | "plum"
    )
}

fn is_valid_accent(accent: &str) -> bool {
    matches!(
        accent,
        "blue" | "indigo" | "purple" | "pink" | "red" | "orange" | "green" | "teal" | "graphite"
    )
}

impl Storage {
    pub fn theme(&self) -> Result<String, String> {
        let value: Option<String> = self
            .connection()?
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'ui_theme'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        Ok(value
            .filter(|theme| is_valid_theme(theme))
            .unwrap_or_else(|| DEFAULT_THEME.into()))
    }

    pub fn set_theme(&self, theme: &str) -> Result<String, String> {
        if !is_valid_theme(theme) {
            return Err("不支持的配色方案".into());
        }
        self.connection()?
            .execute(
                "INSERT INTO app_settings (key, value) VALUES ('ui_theme', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [theme],
            )
            .map_err(|error| error.to_string())?;
        Ok(theme.into())
    }

    pub fn accent(&self) -> Result<String, String> {
        let value: Option<String> = self
            .connection()?
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'ui_accent'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        Ok(value
            .filter(|accent| is_valid_accent(accent))
            .unwrap_or_else(|| DEFAULT_ACCENT.into()))
    }

    pub fn set_accent(&self, accent: &str) -> Result<String, String> {
        if !is_valid_accent(accent) {
            return Err("不支持的强调色".into());
        }
        self.connection()?
            .execute(
                "INSERT INTO app_settings (key, value) VALUES ('ui_accent', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [accent],
            )
            .map_err(|error| error.to_string())?;
        Ok(accent.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::{fs, path::PathBuf};

    struct TestStore {
        directory: PathBuf,
        storage: Storage,
    }

    impl TestStore {
        fn new() -> Self {
            let directory = std::env::temp_dir().join("kedu-tests").join(format!(
                "theme-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap()
            ));
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join(".kedu-test-data"), "").unwrap();
            let storage = Storage::new(directory.join("kedu.sqlite")).unwrap();
            Self { directory, storage }
        }
    }

    impl Drop for TestStore {
        fn drop(&mut self) {
            // Only remove the uniquely created test directory, never a user path.
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn theme_persists_and_collector_settings_preserve_it() {
        let fixture = TestStore::new();
        let storage = &fixture.storage;
        assert_eq!(storage.theme().unwrap(), "light");
        for theme in [
            "light", "dark", "system", "celadon", "paper", "mist", "ink", "midnight", "plum",
        ] {
            assert_eq!(storage.set_theme(theme).unwrap(), theme);
            assert_eq!(storage.theme().unwrap(), theme);
        }
        storage.set_theme("midnight").unwrap();
        storage
            .update_settings(storage.settings().unwrap())
            .unwrap();
        let reopened = Storage::new(fixture.directory.join("kedu.sqlite")).unwrap();
        assert_eq!(reopened.theme().unwrap(), "midnight");
        assert!(storage.set_theme("unknown").is_err());
        assert!(storage.set_theme(" INK ").is_err());
        assert_eq!(storage.theme().unwrap(), "midnight");
    }

    #[test]
    fn theme_survives_backup_and_old_backup_uses_default() {
        let fixture = TestStore::new();
        let storage = &fixture.storage;
        let old_backup = fixture.directory.join("old.sqlite");
        storage.backup_to(old_backup.clone()).unwrap();
        storage.set_theme("plum").unwrap();
        let new_backup = fixture.directory.join("new.sqlite");
        storage.backup_to(new_backup.clone()).unwrap();
        storage.set_theme("paper").unwrap();
        storage.restore_from(new_backup).unwrap();
        assert_eq!(storage.theme().unwrap(), "plum");
        storage.restore_from(old_backup).unwrap();
        assert_eq!(storage.theme().unwrap(), "light");
    }

    #[test]
    fn accent_defaults_validates_and_persists() {
        let fixture = TestStore::new();
        let storage = &fixture.storage;
        assert_eq!(storage.accent().unwrap(), "blue");
        assert_eq!(storage.set_accent("teal").unwrap(), "teal");
        assert!(storage.set_accent("neon").is_err());
        let reopened = Storage::new(fixture.directory.join("kedu.sqlite")).unwrap();
        assert_eq!(reopened.accent().unwrap(), "teal");
    }

    #[test]
    fn unknown_stored_theme_falls_back_without_rewriting_it() {
        let fixture = TestStore::new();
        fixture
            .storage
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO app_settings (key, value) VALUES ('ui_theme', 'future-theme')",
                [],
            )
            .unwrap();
        assert_eq!(fixture.storage.theme().unwrap(), "light");
        let raw: String = fixture
            .storage
            .connection()
            .unwrap()
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'ui_theme'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw, "future-theme");
    }
}
