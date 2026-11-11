//! Verified component ownership. Raw process paths and activity rows stay intact.
pub fn canonical_executable(value: &str) -> String {
    let path = value.trim().trim_matches('"').replace('/', "\\");
    let name = path.rsplit('\\').next().unwrap_or_default().to_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    match stem {
        "steam" | "steamwebhelper" => "steam.exe".into(),
        _ => format!("{stem}.exe"),
    }
}

pub fn display_name(value: &str, fallback: &str) -> String {
    match canonical_executable(value).as_str() {
        "steam.exe" => "Steam".into(),
        "startmenuexperiencehost.exe" => "Windows 开始菜单".into(),
        "uninstall.exe" => "卸载程序".into(),
        _ => fallback.to_string(),
    }
}

pub fn matches(path: &str, name: &str, patterns: &str) -> bool {
    patterns
        .split([';', ','])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .any(|pattern| {
            let identity = if path.is_empty() { name } else { path };
            if canonical_executable(pattern) == "steam.exe" {
                canonical_executable(identity) == "steam.exe"
            } else {
                let pattern = pattern.to_lowercase();
                path.to_lowercase().contains(&pattern)
                    || name
                        .to_lowercase()
                        .contains(pattern.trim_end_matches(".exe"))
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn steam_components_merge_but_games_and_unknown_helpers_do_not() {
        assert_eq!(
            canonical_executable(r"F:\Steam\bin\cef\steamwebhelper.exe"),
            "steam.exe"
        );
        assert!(!matches(
            r"F:\Steam\steamapps\common\Probably Stolen\Probably Stolen.exe",
            "Probably Stolen",
            "steam"
        ));
        assert!(matches(
            r"F:\Steam\bin\steamwebhelper.exe",
            "Steam Client WebHelper",
            "steam.exe"
        ));
        assert_eq!(canonical_executable("myhelper.exe"), "myhelper.exe");
    }
}
