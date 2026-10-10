//! User analysis settings from `config.toml`: title rules and categories. Both
//! are applied at query time over original evidence, so editing the file
//! reinterprets all retained history without rewriting it.
use crate::{categories::Categories, titles::TitleRules};
use anyhow::Result;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    title_rules: Vec<crate::titles::RuleSettings>,
    #[serde(default)]
    categories: Vec<crate::categories::CategorySettings>,
}

#[derive(Default)]
pub struct Settings {
    pub titles: TitleRules,
    pub categories: Categories,
    /// Why the file was ignored, if it was invalid or unreadable.
    pub problem: Option<String>,
}
impl Settings {
    pub fn parse(text: &str) -> Result<Self> {
        let file: File = toml::from_str(text)?;
        Ok(Self {
            titles: TitleRules::new(file.title_rules)?,
            categories: Categories::new(file.categories)?,
            problem: None,
        })
    }

    /// Settings from a file; a missing file has none. Parsed settings are
    /// reused until the file changes. An invalid file is reported once and
    /// ignored as a whole rather than failing every query.
    pub fn load(path: &Path) -> Arc<Self> {
        type Loaded = Option<(PathBuf, Option<(SystemTime, u64)>, Arc<Settings>)>;
        static LOADED: OnceLock<Mutex<Loaded>> = OnceLock::new();
        let stamp = fs::metadata(path)
            .ok()
            .map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()));
        let mut loaded = LOADED.get_or_init(Default::default).lock().unwrap();
        if let Some((cached, cached_stamp, settings)) = loaded.as_ref()
            && cached == path
            && *cached_stamp == stamp
        {
            return settings.clone();
        }
        let settings = Arc::new(Self::read(path).unwrap_or_else(|error| {
            eprintln!("ignoring settings in {}: {error:#}", path.display());
            Self {
                problem: Some(format!("{error:#}")),
                ..Self::default()
            }
        }));
        *loaded = Some((path.into(), stamp, settings.clone()));
        settings
    }
    fn read(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }
}

/// Short content version of one settings section; empty when it has no entries,
/// so histories analysed without settings keep stable identifiers.
pub(crate) fn version<T: std::fmt::Debug>(entries: &[T]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    blake3::hash(format!("{entries:?}").as_bytes()).to_hex()[..12].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_version_independently_and_invalid_files_are_ignored() {
        let titles = "[[title_rules]]\npattern = 'x'\n";
        let categories =
            "[[categories]]\nname = 'Chat'\n[[categories.rules]]\napplication = 'chat'\n";
        let both = Settings::parse(&format!("{titles}{categories}")).unwrap();
        let only_titles = Settings::parse(titles).unwrap();
        assert_eq!(both.titles.version(), only_titles.titles.version());
        assert_eq!(only_titles.categories.version(), "");
        assert_eq!(both.categories.version().len(), 12);

        let missing = Settings::load(Path::new("/nonexistent/timeline/config.toml"));
        assert!(missing.problem.is_none());
        assert_eq!(missing.categories.version(), "");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[[categories]]\nname = 'Chat'\ncolour = 'red'\n").unwrap();
        let invalid = Settings::load(&path);
        assert!(invalid.problem.as_deref().unwrap().contains("colour"));
        assert!(invalid.categories.list().is_empty());
        fs::write(&path, categories).unwrap();
        let reloaded = Settings::load(&path);
        assert!(reloaded.problem.is_none());
        assert_eq!(reloaded.categories.list()[0].name, "Chat");
    }
}
