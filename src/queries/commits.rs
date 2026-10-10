//! The user's git commits as external evidence beside recorded activity. They
//! are read from the configured repositories at query time, never recorded or
//! retained, and are not task attribution.
use super::{Client, PAGE_BYTES, Queries, QueryError, ResolvedRange, Selection};
use crate::git::{self, Commit, Repository, RepositoryProblem};
use anyhow::{Result, bail};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CommitDay {
    /// Activity date, `YYYY-MM-DD`.
    pub date: String,
    pub range: super::Range,
    pub commits: usize,
}

#[derive(Debug, Serialize)]
pub struct CommitList {
    pub resolved: ResolvedRange,
    /// Whether the settings name any paths to search for repositories.
    pub configured: bool,
    /// Commits made in the range, ordered by time.
    pub commits: Vec<Commit>,
    /// Commits per activity day of the range, each clipped to it.
    pub days: Vec<CommitDay>,
    /// Repositories searched.
    pub repositories: usize,
    /// Configured paths and repositories that could not be read; their
    /// commits are missing, not absent.
    pub problems: Vec<RepositoryProblem>,
    /// Why the settings file was ignored, if it was invalid.
    pub settings_problem: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RepositoryList {
    #[serde(flatten)]
    pub sources: git::GitSources,
    pub repositories: Vec<Repository>,
    pub problems: Vec<RepositoryProblem>,
    pub scanned_ms: Option<i64>,
    pub directories: usize,
    pub truncated: bool,
    pub settings_problem: Option<String>,
}

impl Queries {
    /// The user's commits in a selection of at most 31 days, from repositories
    /// under the configured git paths, with per-day counts.
    pub fn commits(&self, client: Client, selection: Selection) -> Result<CommitList> {
        let resolved = self.resolve(selection)?;
        let range = resolved.range;
        let mut days: Vec<CommitDay> = self
            .activity_days(range)?
            .into_iter()
            .map(|(date, range)| CommitDay {
                date: date.to_string(),
                range,
                commits: 0,
            })
            .collect();
        let sources = &self.settings.git;
        let mut list = CommitList {
            resolved,
            configured: sources.configured(),
            commits: Vec::new(),
            days: Vec::new(),
            repositories: 0,
            problems: Vec::new(),
            settings_problem: self.settings.problem.clone(),
        };
        if list.configured {
            let _permit = self.admit(client)?;
            let discovery = git::discover(sources, false);
            let reading = git::read(&discovery, sources, range.start_ms, range.end_ms, || {
                self.check_cancelled()
            })?;
            list.repositories = discovery.repositories.len();
            list.problems = discovery.problems.clone();
            list.problems.extend(reading.problems);
            list.commits = reading.commits;
            for commit in &list.commits {
                let index = days.partition_point(|day| day.range.end_ms <= commit.time_ms);
                if let Some(day) = days.get_mut(index) {
                    day.commits += 1;
                }
            }
        }
        list.days = days;
        if serde_json::to_vec(&list.commits)?.len() > PAGE_BYTES {
            bail!(QueryError::LimitExceeded);
        }
        Ok(list)
    }

    /// The configured git paths and the repositories found under them;
    /// `refresh` searches again instead of reusing a recent search.
    pub fn repositories(&self, refresh: bool) -> RepositoryList {
        let sources = &self.settings.git;
        let mut list = RepositoryList {
            sources: sources.clone(),
            repositories: Vec::new(),
            problems: Vec::new(),
            scanned_ms: None,
            directories: 0,
            truncated: false,
            settings_problem: self.settings.problem.clone(),
        };
        if sources.configured() {
            let discovery = git::discover(sources, refresh);
            list.repositories = discovery.repositories.clone();
            list.problems = discovery.problems.clone();
            list.scanned_ms = Some(discovery.scanned_ms);
            list.directories = discovery.directories;
            list.truncated = discovery.truncated;
        }
        list
    }
}
