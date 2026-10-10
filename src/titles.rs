//! User rules that clean window titles before activity analysis. Recorded
//! evidence keeps the raw title; only derived focus coverage is cleaned.
use crate::recording::Window;
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    #[serde(default)]
    title_rules: Vec<RuleSettings>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleSettings {
    /// Exact application (window class); every application when absent.
    application: Option<String>,
    pattern: String,
    #[serde(default)]
    replacement: String,
}

struct Rule {
    application: Option<String>,
    pattern: Regex,
    replacement: String,
}

#[derive(Default)]
pub struct TitleRules {
    rules: Vec<Rule>,
    /// Empty without rules, so raw-title block IDs and cursors stay stable.
    version: String,
}
impl TitleRules {
    pub fn parse(text: &str) -> Result<Self> {
        let settings: Settings = toml::from_str(text)?;
        let rules = settings
            .title_rules
            .into_iter()
            .map(|rule| {
                Ok(Rule {
                    pattern: Regex::new(&rule.pattern)
                        .with_context(|| format!("invalid title pattern {:?}", rule.pattern))?,
                    application: rule.application,
                    replacement: rule.replacement,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let version = if rules.is_empty() {
            String::new()
        } else {
            blake3::hash(text.as_bytes()).to_hex()[..12].to_owned()
        };
        Ok(Self { rules, version })
    }

    /// Rules from a settings file; a missing file has none. Parsed rules are
    /// reused until the file changes. An invalid file is reported once and
    /// leaves titles raw rather than failing every query.
    pub fn load(path: &Path) -> Arc<Self> {
        type Loaded = Option<(PathBuf, Option<(SystemTime, u64)>, Arc<TitleRules>)>;
        static LOADED: OnceLock<Mutex<Loaded>> = OnceLock::new();
        let stamp = fs::metadata(path)
            .ok()
            .map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()));
        let mut loaded = LOADED.get_or_init(Default::default).lock().unwrap();
        if let Some((cached, cached_stamp, rules)) = loaded.as_ref()
            && cached == path
            && *cached_stamp == stamp
        {
            return rules.clone();
        }
        let rules = Arc::new(Self::read(path).unwrap_or_else(|error| {
            eprintln!("ignoring title rules in {}: {error:#}", path.display());
            Self::default()
        }));
        *loaded = Some((path.into(), stamp, rules.clone()));
        rules
    }
    fn read(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Applies every matching rule in order, then trims surrounding whitespace.
    pub fn clean(&self, window: &Window) -> Window {
        let mut title = window.title.clone();
        let mut changed = false;
        for rule in &self.rules {
            if rule
                .application
                .as_ref()
                .is_some_and(|application| *application != window.application)
            {
                continue;
            }
            if let std::borrow::Cow::Owned(cleaned) =
                rule.pattern.replace_all(&title, rule.replacement.as_str())
            {
                title = cleaned;
                changed = true;
            }
        }
        Window {
            title: if changed { title.trim().into() } else { title },
            ..window.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(application: &str, title: &str) -> Window {
        Window {
            address: "1".into(),
            application: application.into(),
            title: title.into(),
        }
    }

    #[test]
    fn rules_clean_matching_applications_in_order() {
        let rules = TitleRules::parse(
            r#"
            [[title_rules]]
            pattern = '^[\x{2800}-\x{28FF}◐◑✳]'

            [[title_rules]]
            application = "code"
            pattern = '^●'

            [[title_rules]]
            pattern = '(\d+) new'
            replacement = 'N new'
            "#,
        )
        .unwrap();
        let clean = |application, title| rules.clean(&window(application, title)).title;
        assert_eq!(clean("terminal", "⠋ Review"), "Review");
        assert_eq!(clean("terminal", "◑ Review"), "Review");
        assert_eq!(clean("code", "● main.rs - repo"), "main.rs - repo");
        assert_eq!(clean("browser", "● main.rs"), "● main.rs");
        assert_eq!(clean("chat", "Inbox (3 new) "), "Inbox (N new)");
        assert_eq!(clean("chat", " Untouched "), " Untouched ");
        assert_eq!(rules.version().len(), 12);
    }

    #[test]
    fn missing_or_empty_settings_keep_raw_titles() {
        let rules = TitleRules::load(Path::new("/nonexistent/timeline/config.toml"));
        assert_eq!(rules.version(), "");
        assert_eq!(rules.clean(&window("a", "⠋ x")).title, "⠋ x");
        assert_eq!(TitleRules::parse("").unwrap().version(), "");
        assert!(TitleRules::parse("[[title_rules]]\npattern = '('").is_err());
        assert!(TitleRules::parse("[[title_rule]]\npattern = 'x'").is_err());
    }
}
