//! User rules that clean window titles before activity analysis. Recorded
//! evidence keeps the raw title; only derived focus coverage is cleaned.
use crate::recording::Window;
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;

/// One `[[title_rules]]` entry of the settings file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuleSettings {
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
    pub(crate) fn new(settings: Vec<RuleSettings>) -> Result<Self> {
        let version = crate::settings::version(&settings);
        let rules = settings
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
        Ok(Self { rules, version })
    }

    /// Title rules of a complete settings file.
    pub fn parse(text: &str) -> Result<Self> {
        Ok(crate::settings::Settings::parse(text)?.titles)
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
    fn empty_or_invalid_rules_keep_raw_titles() {
        assert_eq!(
            TitleRules::default().clean(&window("a", "⠋ x")).title,
            "⠋ x"
        );
        assert_eq!(TitleRules::parse("").unwrap().version(), "");
        assert!(TitleRules::parse("[[title_rules]]\npattern = '('").is_err());
        assert!(TitleRules::parse("[[title_rule]]\npattern = 'x'").is_err());
    }
}
