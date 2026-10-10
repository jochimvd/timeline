//! User-defined categories that classify focus time by application and cleaned
//! window title, e.g. "Communication" or a project. A category is a derived
//! view over original evidence, not task attribution: nothing is persisted, and
//! editing the rules re-categorizes all retained history. A category can name
//! a `parent`, nesting it as a subcategory: rules still match in configuration
//! order, and the hierarchy only decides how time rolls up and colors carry down.
use crate::recording::Window;
use anyhow::{Context, Result, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level categories without an explicit color take one of this many
/// palette slots, in configuration order. Later ones share the neutral
/// uncategorized color.
pub const PALETTE_SLOTS: u8 = 8;
/// Nesting levels, counting top-level categories as the first.
pub const MAX_DEPTH: usize = 4;

/// One `[[categories]]` entry of the settings file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CategorySettings {
    name: String,
    /// Name of the enclosing category, making this one a subcategory.
    parent: Option<String>,
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
    /// In tree order: each category followed by its subcategories.
    categories: Vec<Category>,
    /// Index of each category's parent in `categories`.
    parents: Vec<Option<usize>>,
    /// In configuration order, independent of the hierarchy.
    matchers: Vec<Matcher>,
    /// Empty without categories, so uncategorized cursors stay stable.
    version: String,
}
impl Categories {
    pub(crate) fn new(settings: Vec<CategorySettings>) -> Result<Self> {
        let version = crate::settings::version(&settings);
        let mut index = HashMap::new();
        for (position, category) in settings.iter().enumerate() {
            let name = category.name.trim();
            ensure!(
                !name.is_empty(),
                "category {} has an empty name",
                position + 1
            );
            ensure!(
                index.insert(name, position).is_none(),
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
        }
        let parents = settings
            .iter()
            .map(|category| {
                let Some(parent) = &category.parent else {
                    return Ok(None);
                };
                let name = category.name.trim();
                let parent = *index.get(parent.trim()).with_context(|| {
                    format!("category {name:?} has an undefined parent {parent:?}")
                })?;
                Ok(Some(parent))
            })
            .collect::<Result<Vec<_>>>()?;
        for (position, category) in settings.iter().enumerate() {
            let mut ancestor = parents[position];
            for _ in 1..MAX_DEPTH {
                let Some(next) = ancestor else { break };
                ensure!(
                    next != position,
                    "category {:?} is its own ancestor",
                    category.name.trim()
                );
                ancestor = parents[next];
            }
            ensure!(
                ancestor.is_none(),
                "category {:?} is nested more than {MAX_DEPTH} levels deep",
                category.name.trim()
            );
        }

        // Depth-first from top-level categories, siblings in configuration order.
        let mut order = Vec::with_capacity(settings.len());
        let mut pending: Vec<usize> = (0..settings.len())
            .rev()
            .filter(|&position| parents[position].is_none())
            .collect();
        while let Some(position) = pending.pop() {
            order.push(position);
            pending.extend(
                (0..settings.len())
                    .rev()
                    .filter(|&child| parents[child] == Some(position)),
            );
        }
        let mut placed = vec![0; settings.len()];
        for (tree, &position) in order.iter().enumerate() {
            placed[position] = tree;
        }

        let mut result = Self {
            version,
            ..Self::default()
        };
        for (position, category) in settings.iter().enumerate() {
            let name = category.name.trim();
            for rule in &category.rules {
                ensure!(
                    rule.application.is_some() || rule.title.is_some(),
                    "a rule of category {name:?} needs an application or a title"
                );
                result.matchers.push(Matcher {
                    category: placed[position],
                    application: rule.application.clone(),
                    title: rule
                        .title
                        .as_deref()
                        .map(Regex::new)
                        .transpose()
                        .with_context(|| format!("invalid title pattern in category {name:?}"))?,
                });
            }
        }
        let mut settings: Vec<_> = settings.into_iter().map(Some).collect();
        let mut slots = 0;
        for &position in &order {
            let category = settings[position].take().expect("each category once");
            let parent = parents[position].map(|parent| placed[parent]);
            // Only top-level categories take palette slots; subcategories
            // carry their parent's color unless they set their own.
            let (color, palette, depth) = match parent.map(|parent| &result.categories[parent]) {
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
            result.categories.push(Category {
                name: category.name.trim().to_owned(),
                parent: parent.map(|parent| result.categories[parent].name.clone()),
                depth,
                palette: if color.is_some() { None } else { palette },
                color,
                rules: category.rules,
            });
            result.parents.push(parent);
        }
        Ok(result)
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Configured categories in tree order: each category followed by its
    /// subcategories, siblings in configuration order.
    pub fn list(&self) -> &[Category] {
        &self.categories
    }

    /// Positions in `list` of the named category and its ancestors, innermost first.
    pub fn lineage(&self, name: &str) -> impl Iterator<Item = usize> + '_ {
        let start = self.categories.iter().position(|c| c.name == name);
        std::iter::successors(start, |&position| self.parents[position])
    }

    /// The category of the first matching rule in configuration order,
    /// whatever its place in the hierarchy.
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
    fn subcategories_nest_in_tree_order_but_rules_match_in_file_order() {
        let settings = Settings::parse(
            r##"
            [[categories]]
            name = "Project X"
            parent = "Acme"
            [[categories.rules]]
            title = 'project-x'

            [[categories]]
            name = "Work"
            [[categories.rules]]
            title = 'Jira'

            [[categories]]
            name = "Personal"
            color = "#2fb344"

            [[categories]]
            name = "Acme"
            parent = "Work"
            [[categories.rules]]
            application = "teams"

            [[categories]]
            name = "Dotfiles"
            parent = "Personal"
            [[categories.rules]]
            title = 'dotfiles'
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
                ("Work", None, 0),
                ("Acme", Some("Work"), 1),
                ("Project X", Some("Acme"), 2),
                ("Personal", None, 0),
                ("Dotfiles", Some("Personal"), 1),
            ]
        );
        // Subcategories carry their top-level color or palette slot.
        assert_eq!(
            list.iter()
                .map(|c| (c.color.as_deref(), c.palette))
                .collect::<Vec<_>>(),
            [
                (None, Some(1)),
                (None, Some(1)),
                (None, Some(1)),
                (Some("#2fb344"), None),
                (Some("#2fb344"), None),
            ]
        );
        let name = |title| {
            categories
                .classify(&window("browser", title))
                .map(|c| c.name.as_str())
        };
        // "Project X" is first in the file, so it wins over its ancestor's rule.
        assert_eq!(name("Jira - project-x"), Some("Project X"));
        assert_eq!(name("Jira board"), Some("Work"));
        assert_eq!(name("dotfiles"), Some("Dotfiles"));
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
            "[[categories]]\nname = 'A'\nparent = 'A'",
            "[[categories]]\nname = 'A'\nparent = 'B'\n[[categories]]\nname = 'B'\nparent = 'A'",
            "[[categories]]\nname = 'A'\n[[categories]]\nname = 'B'\nparent = 'A'\n\
             [[categories]]\nname = 'C'\nparent = 'B'\n[[categories]]\nname = 'D'\nparent = 'C'\n\
             [[categories]]\nname = 'E'\nparent = 'D'",
        ] {
            assert!(Settings::parse(text).is_err(), "{text}");
        }
    }
}
