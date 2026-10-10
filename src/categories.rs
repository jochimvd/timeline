//! User-defined categories that classify focus time by application and cleaned
//! window title, e.g. "Communication" or a project. A category is a derived
//! view over original evidence, not task attribution: nothing is persisted, and
//! editing the rules re-categorizes all retained history. Categories nest as
//! `children`: rules match in file order, the hierarchy decides how time rolls
//! up and colors carry down.
use crate::recording::Window;
use anyhow::{Context, Result, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Top-level categories without an explicit color take one of this many
/// palette slots, in configuration order. Later ones share the neutral
/// uncategorized color.
pub const PALETTE_SLOTS: u8 = 8;
/// Nesting levels, counting top-level categories as the first.
pub const MAX_DEPTH: usize = 4;

/// One `[[categories]]` entry of the settings file, with its subcategories.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CategorySettings {
    name: String,
    color: Option<String>,
    #[serde(default)]
    rules: Vec<CategoryRule>,
    #[serde(default)]
    children: Vec<CategorySettings>,
}

/// Matches when every given field matches: `application` is an exact window
/// class and `title` a regex searched in the cleaned title. Fallback rules are
/// tried only after every other rule, for broad guesses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fallback: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Category {
    pub name: String,
    /// Enclosing category of a subcategory.
    pub parent: Option<String>,
    /// Nesting level: 0 for top-level categories.
    pub depth: usize,
    /// `#rrggbb` color: the configured one, else the nearest ancestor's.
    pub color: Option<String>,
    /// 1-based categorical palette slot when no color applies, shared by a
    /// top-level category's subcategories.
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
    /// In file order, which is tree order: each category followed by its
    /// subcategories.
    categories: Vec<Category>,
    /// Index of each category's parent in `categories`.
    parents: Vec<Option<usize>>,
    /// Regular rules in file order, then fallback rules in file order.
    matchers: Vec<Matcher>,
    /// Empty without categories, so uncategorized cursors stay stable.
    version: String,
}
impl Categories {
    pub(crate) fn new(settings: Vec<CategorySettings>) -> Result<Self> {
        let mut result = Self {
            version: crate::settings::version(&settings),
            ..Self::default()
        };
        let mut names = HashSet::new();
        let mut fallbacks = Vec::new();
        let mut slots = 0;
        // Depth-first, so each category precedes its subcategories.
        let mut pending: Vec<_> = settings.into_iter().rev().map(|c| (c, None)).collect();
        while let Some((category, parent)) = pending.pop() {
            let name = category.name.trim().to_owned();
            ensure!(!name.is_empty(), "a category has an empty name");
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
            let position = result.categories.len();
            for rule in &category.rules {
                ensure!(
                    rule.application.is_some() || rule.title.is_some(),
                    "a rule of category {name:?} needs an application or a title"
                );
                let matcher = Matcher {
                    category: position,
                    application: rule.application.clone(),
                    title: rule
                        .title
                        .as_deref()
                        .map(Regex::new)
                        .transpose()
                        .with_context(|| format!("invalid title pattern in category {name:?}"))?,
                };
                if rule.fallback {
                    fallbacks.push(matcher);
                } else {
                    result.matchers.push(matcher);
                }
            }
            // Only top-level categories take palette slots; subcategories
            // carry their parent's color unless they set their own.
            let (color, palette, depth) = match parent.map(|p: usize| &result.categories[p]) {
                Some(parent) => (
                    category.color.or_else(|| parent.color.clone()),
                    parent.palette,
                    parent.depth + 1,
                ),
                None => {
                    let palette = (category.color.is_none() && slots < PALETTE_SLOTS).then(|| {
                        slots += 1;
                        slots
                    });
                    (category.color, palette, 0)
                }
            };
            ensure!(
                depth < MAX_DEPTH,
                "category {name:?} is nested more than {MAX_DEPTH} levels deep"
            );
            result.categories.push(Category {
                parent: parent.map(|p| result.categories[p].name.clone()),
                name,
                depth,
                palette: if color.is_some() { None } else { palette },
                color,
                rules: category.rules,
            });
            result.parents.push(parent);
            pending.extend(
                category
                    .children
                    .into_iter()
                    .rev()
                    .map(|child| (child, Some(position))),
            );
        }
        result.matchers.extend(fallbacks);
        Ok(result)
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Configured categories in file order: each category followed by its
    /// subcategories.
    pub fn list(&self) -> &[Category] {
        &self.categories
    }

    /// Positions in `list` of the named category and its ancestors, innermost first.
    pub fn lineage(&self, name: &str) -> impl Iterator<Item = usize> + '_ {
        let start = self.categories.iter().position(|c| c.name == name);
        std::iter::successors(start, |&position| self.parents[position])
    }

    /// The category of the first matching rule in file order, whatever its
    /// place in the hierarchy, trying fallback rules last.
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
    fn nested_categories_match_in_file_order_with_fallback_rules_last() {
        let settings = Settings::parse(
            r##"
            [[categories]]
            name = "Personal"
            color = "#2fb344"
              [[categories.children]]
              name = "Dotfiles"
              rules = [{ title = 'dotfiles' }]

            [[categories]]
            name = "Work"
            rules = [{ title = 'Jira' }]
              [[categories.children]]
              name = "Acme"
              rules = [{ application = "teams" }]
                [[categories.children.children]]
                name = "Project X"
                rules = [
                  { title = 'project-x' },
                  { application = "terminal", title = 'Review', fallback = true },
                ]
              [[categories.children]]
              name = "Globex"
              rules = [{ application = "terminal", title = 'monorepo' }]
            "##,
        )
        .unwrap();
        let categories = &settings.categories;
        let list = categories.list();
        assert_eq!(
            list.iter()
                .map(|c| (c.name.as_str(), c.parent.as_deref(), c.depth))
                .collect::<Vec<_>>(),
            [
                ("Personal", None, 0),
                ("Dotfiles", Some("Personal"), 1),
                ("Work", None, 0),
                ("Acme", Some("Work"), 1),
                ("Project X", Some("Acme"), 2),
                ("Globex", Some("Work"), 1),
            ]
        );
        // Subcategories carry their top-level color or palette slot.
        assert_eq!(
            list.iter()
                .map(|c| (c.color.as_deref(), c.palette))
                .collect::<Vec<_>>(),
            [
                (Some("#2fb344"), None),
                (Some("#2fb344"), None),
                (None, Some(1)),
                (None, Some(1)),
                (None, Some(1)),
                (None, Some(1)),
            ]
        );
        let name = |application, title| {
            categories
                .classify(&window(application, title))
                .map(|c| c.name.as_str())
        };
        // A parent's rule earlier in the file wins over a subcategory's.
        assert_eq!(name("browser", "Jira - project-x"), Some("Work"));
        assert_eq!(name("browser", "project-x"), Some("Project X"));
        // The fallback rule only applies once no other rule matches.
        assert_eq!(name("terminal", "Review monorepo"), Some("Globex"));
        assert_eq!(name("terminal", "Review"), Some("Project X"));
        assert_eq!(
            categories
                .lineage("Project X")
                .map(|i| list[i].name.as_str())
                .collect::<Vec<_>>(),
            ["Project X", "Acme", "Work"]
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
            "[[categories]]\nname = 'A'\nparent = 'B'",
            "[[categories]]\nname = 'A'\n[[categories.children]]\nname = 'A'",
            "[[categories]]\nname = 'A'\n[[categories.children]]\nname = 'B'\n\
             [[categories.children.children]]\nname = 'C'\n\
             [[categories.children.children.children]]\nname = 'D'\n\
             [[categories.children.children.children.children]]\nname = 'E'",
        ] {
            assert!(Settings::parse(text).is_err(), "{text}");
        }
    }
}
