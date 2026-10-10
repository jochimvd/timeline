//! User-defined categories that classify focus time by application and cleaned
//! window title, e.g. "Communication" or a project. A category is a derived
//! view over original evidence, not task attribution: nothing is persisted, and
//! editing the rules re-categorizes all retained history.
use crate::recording::Window;
use anyhow::{Context, Result, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Categories without an explicit color take one of this many palette slots,
/// in configuration order. Later ones share the neutral uncategorized color.
pub const PALETTE_SLOTS: u8 = 8;

/// One `[[categories]]` entry of the settings file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CategorySettings {
    name: String,
    color: Option<String>,
    #[serde(default)]
    rules: Vec<CategoryRule>,
}

/// Matches when every given field matches: `application` is an exact window
/// class and `title` a regex searched in the cleaned title.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Category {
    pub name: String,
    /// Explicit `#rrggbb` color from the settings.
    pub color: Option<String>,
    /// 1-based categorical palette slot when no color is configured.
    pub palette: Option<u8>,
    pub rules: Vec<CategoryRule>,
}

struct Matcher {
    category: usize,
    application: Option<String>,
    title: Option<Regex>,
}

#[derive(Default)]
pub struct Categories {
    categories: Vec<Category>,
    matchers: Vec<Matcher>,
    /// Empty without categories, so uncategorized cursors stay stable.
    version: String,
}
impl Categories {
    pub(crate) fn new(settings: Vec<CategorySettings>) -> Result<Self> {
        let version = crate::settings::version(&settings);
        let mut names = HashSet::new();
        let mut slots = 0;
        let mut result = Self {
            version,
            ..Self::default()
        };
        for (index, category) in settings.into_iter().enumerate() {
            let name = category.name.trim().to_owned();
            ensure!(!name.is_empty(), "category {} has an empty name", index + 1);
            ensure!(
                names.insert(name.clone()),
                "category {name:?} is defined twice"
            );
            if let Some(color) = &category.color {
                ensure!(
                    color.len() == 7
                        && color.starts_with('#')
                        && color[1..].bytes().all(|b| b.is_ascii_hexdigit()),
                    "category {name:?} color must look like #4f8cff"
                );
            }
            for rule in &category.rules {
                ensure!(
                    rule.application.is_some() || rule.title.is_some(),
                    "a rule of category {name:?} needs an application or a title"
                );
                result.matchers.push(Matcher {
                    category: index,
                    application: rule.application.clone(),
                    title: rule
                        .title
                        .as_deref()
                        .map(Regex::new)
                        .transpose()
                        .with_context(|| format!("invalid title pattern in category {name:?}"))?,
                });
            }
            let palette = (category.color.is_none() && slots < PALETTE_SLOTS).then(|| {
                slots += 1;
                slots
            });
            result.categories.push(Category {
                name,
                color: category.color,
                palette,
                rules: category.rules,
            });
        }
        Ok(result)
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Configured categories in configuration order.
    pub fn list(&self) -> &[Category] {
        &self.categories
    }

    /// The first category, in configuration order, with a matching rule.
    pub fn classify(&self, window: &Window) -> Option<&Category> {
        self.matchers
            .iter()
            .find(|matcher| {
                matcher
                    .application
                    .as_ref()
                    .is_none_or(|application| *application == window.application)
                    && matcher
                        .title
                        .as_ref()
                        .is_none_or(|title| title.is_match(&window.title))
            })
            .map(|matcher| &self.categories[matcher.category])
    }
}

#[cfg(test)]
mod tests {
    use crate::settings::Settings;

    fn window(application: &str, title: &str) -> crate::recording::Window {
        crate::recording::Window {
            address: "1".into(),
            application: application.into(),
            title: title.into(),
        }
    }

    #[test]
    fn first_matching_category_in_configuration_order_wins() {
        let settings = Settings::parse(
            r##"
            [[categories]]
            name = "Timeline"
            color = "#4f8cff"
            [[categories.rules]]
            application = "code"
            title = '(?i)timeline'
            [[categories.rules]]
            title = 'localhost:47831'

            [[categories]]
            name = " Coding "
            [[categories.rules]]
            application = "code"

            [[categories]]
            name = "Browsing"
            [[categories.rules]]
            application = "firefox"
            "##,
        )
        .unwrap();
        let categories = &settings.categories;
        let name = |application, title| {
            categories
                .classify(&window(application, title))
                .map(|c| c.name.as_str())
        };
        assert_eq!(name("code", "queries.rs - Timeline"), Some("Timeline"));
        assert_eq!(name("code", "other.rs - repo"), Some("Coding"));
        assert_eq!(name("firefox", "localhost:47831"), Some("Timeline"));
        assert_eq!(name("firefox", "News"), Some("Browsing"));
        assert_eq!(name("chat", "Inbox"), None);
        let list = categories.list();
        assert_eq!(
            list.iter()
                .map(|c| (c.color.as_deref(), c.palette))
                .collect::<Vec<_>>(),
            [(Some("#4f8cff"), None), (None, Some(1)), (None, Some(2))]
        );
    }

    #[test]
    fn invalid_categories_are_rejected() {
        for text in [
            "[[categories]]\nname = ' '",
            "[[categories]]\nname = 'A'\n[[categories]]\nname = 'A'",
            "[[categories]]\nname = 'A'\ncolor = 'blue'",
            "[[categories]]\nname = 'A'\n[[categories.rules]]",
            "[[categories]]\nname = 'A'\n[[categories.rules]]\ntitle = '('",
            "[[categories]]\nname = 'A'\n[[categories.rules]]\nclass = 'x'",
        ] {
            assert!(Settings::parse(text).is_err(), "{text}");
        }
    }
}
