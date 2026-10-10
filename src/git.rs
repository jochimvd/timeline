//! The user's own git commits in repositories under configured paths, as
//! external evidence read at query time. Nothing is recorded or retained:
//! commits come from each repository's reflogs and objects whenever they are
//! asked for, and repositories are never modified.
//!
//! Timing comes from the reflog, which records when a commit was made in this
//! clone; author and committer dates can be days off after rebases and
//! cherry-picks. Counted reflog events create commits here: `commit` (including
//! initial, amend, merge and cherry-pick conflict resolutions), `cherry-pick`,
//! `revert`, `am`, and `merge`/`pull` that made a merge commit. Rebases, resets,
//! checkouts, fetches and fast-forwards only move existing commits and are not
//! counted. An amend replaces the commit it amended: the commit appears when
//! its final version was made and reports when the original was. A commit seen in several reflogs (HEAD, its branch, worktrees) counts
//! once. A repository without reflogs falls back to committer dates.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

/// Directory levels below a configured path that are searched for repositories.
pub const DEFAULT_DEPTH: usize = 8;
const MAX_DEPTH: usize = 16;
/// Directories one discovery visits at most, across all configured paths.
const DIRECTORY_BUDGET: usize = 50_000;
/// Dependency, build and environment directories that never hold the user's repositories.
const SKIPPED: &[&str] = &[
    "node_modules",
    "bower_components",
    "target",
    "venv",
    "site-packages",
    "__pycache__",
];
/// Discovered repositories and their identities are reused for this long.
const REFRESH: Duration = Duration::from_secs(300);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
/// Time for reading all repositories of one query; the rest are reported skipped.
const READ_BUDGET: Duration = Duration::from_secs(8);
/// Only the newest part of a larger reflog is read.
const REFLOG_BYTES: u64 = 8 * 1024 * 1024;
const OBJECT_BYTES: usize = 16 * 1024 * 1024;
const FALLBACK_COMMITS: usize = 2000;
const SUBJECT_CHARS: usize = 200;
const MAX_PROBLEMS: usize = 100;

/// The `[git]` section of the settings file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitSettings {
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    authors: Vec<String>,
    depth: Option<usize>,
}

/// Where to look for repositories and which extra author emails are the user's.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GitSources {
    /// Configured paths as written in the settings file.
    pub paths: Vec<String>,
    #[serde(skip)]
    roots: Vec<PathBuf>,
    /// Lowercased extra author emails, beyond each repository's `user.email`.
    pub authors: Vec<String>,
    pub depth: usize,
}
impl GitSources {
    /// `~` expands to `home`; other paths must be absolute.
    pub(crate) fn new(settings: GitSettings, home: Option<&Path>) -> Result<Self> {
        let depth = settings.depth.unwrap_or(DEFAULT_DEPTH);
        ensure!(
            (1..=MAX_DEPTH).contains(&depth),
            "git depth must be 1–{MAX_DEPTH}"
        );
        let mut roots = Vec::new();
        for path in &settings.paths {
            let expanded = match path.strip_prefix('~') {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => home
                    .context("git paths starting with ~ need HOME")?
                    .join(rest.trim_start_matches('/')),
                Some(_) => bail!("git path {path:?}: only ~ and ~/… are expanded"),
                None => PathBuf::from(path),
            };
            ensure!(
                expanded.is_absolute(),
                "git path {path:?} must be absolute or start with ~/"
            );
            roots.push(expanded);
        }
        let mut authors = Vec::new();
        for author in &settings.authors {
            let author = author.trim().to_lowercase();
            ensure!(
                author.contains('@') && !author.contains(char::is_whitespace),
                "git author {author:?} must be an email address"
            );
            authors.push(author);
        }
        Ok(Self {
            paths: settings.paths,
            roots,
            authors,
            depth,
        })
    }
    pub fn configured(&self) -> bool {
        !self.roots.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Repository {
    /// The work tree relative to the configured path it was found under.
    pub name: String,
    /// The main work tree, or the work tree where it was found when the main
    /// one is elsewhere.
    pub path: PathBuf,
    /// Common git directory shared by all work trees: the repository's identity.
    #[serde(skip)]
    git_dir: PathBuf,
}

/// A repository, or a configured path, that could not be read.
#[derive(Debug, Clone, Serialize)]
pub struct RepositoryProblem {
    pub repository: String,
    pub problem: String,
}

#[derive(Debug, Serialize)]
pub struct Discovery {
    pub repositories: Vec<Repository>,
    pub problems: Vec<RepositoryProblem>,
    /// When the configured paths were searched.
    pub scanned_ms: i64,
    pub directories: usize,
    /// The directory budget ended the search early.
    pub truncated: bool,
}

#[derive(Default)]
struct Problems(Vec<RepositoryProblem>);
impl Problems {
    fn push(&mut self, repository: String, problem: String) {
        if self.0.len() < MAX_PROBLEMS {
            self.0.push(RepositoryProblem {
                repository,
                problem,
            });
        }
    }
}

fn unix_ms(time: SystemTime) -> i64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Repositories under the configured paths, searched again after five minutes,
/// when the paths change, or on `refresh`.
pub fn discover(sources: &GitSources, refresh: bool) -> Arc<Discovery> {
    type Cached = Option<((Vec<PathBuf>, usize), Instant, Arc<Discovery>)>;
    static CACHE: OnceLock<Mutex<Cached>> = OnceLock::new();
    let key = (sources.roots.clone(), sources.depth);
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    if let Some((cached, at, discovery)) = cache.as_ref()
        && *cached == key
        && !refresh
        && at.elapsed() < REFRESH
    {
        return discovery.clone();
    }
    let discovery = Arc::new(scan(sources));
    *cache = Some((key, Instant::now(), discovery.clone()));
    discovery
}

fn relative(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rest) if !rest.as_os_str().is_empty() => rest.to_string_lossy().into_owned(),
        _ => root.file_name().map_or_else(
            || root.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
    }
}

/// Walks without following symbolic links, so links cannot loop, skipping
/// hidden and dependency directories. Repositories nested in other work trees
/// are found; a repository reached through several work trees is listed once.
fn scan(sources: &GitSources) -> Discovery {
    let mut repositories: Vec<Repository> = Vec::new();
    let mut known: HashMap<PathBuf, usize> = HashMap::new();
    let mut problems = Problems::default();
    let mut directories = 0;
    let mut truncated = false;
    for (configured, root) in sources.paths.iter().zip(&sources.roots) {
        let root = match fs::canonicalize(root) {
            Ok(root) if root.is_dir() => root,
            Ok(_) => {
                problems.push(configured.clone(), "not a directory".into());
                continue;
            }
            Err(error) => {
                problems.push(configured.clone(), format!("cannot read: {error}"));
                continue;
            }
        };
        let mut stack = vec![(root.clone(), 0)];
        while let Some((directory, depth)) = stack.pop() {
            if directories == DIRECTORY_BUDGET {
                truncated = true;
                break;
            }
            directories += 1;
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) => {
                    problems.push(
                        relative(&root, &directory),
                        format!("cannot read directory: {error}"),
                    );
                    continue;
                }
            };
            let mut children = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name == ".git" {
                    // An empty directory, such as a container mount point, is no repository.
                    if fs::read_dir(entry.path()).is_ok_and(|mut entries| entries.next().is_none())
                    {
                        continue;
                    }
                    match repository(&root, &directory, &entry.path()) {
                        Ok(found) => match known.get(&found.git_dir) {
                            // Prefer the main work tree over other work trees.
                            Some(&index) => {
                                if found.git_dir.parent() == Some(found.path.as_path()) {
                                    repositories[index] = found;
                                }
                            }
                            None => {
                                known.insert(found.git_dir.clone(), repositories.len());
                                repositories.push(found);
                            }
                        },
                        Err(error) => {
                            problems.push(relative(&root, &directory), format!("{error:#}"))
                        }
                    }
                    continue;
                }
                let bytes = name.as_encoded_bytes();
                if depth >= sources.depth
                    || bytes.first() == Some(&b'.')
                    || SKIPPED.iter().any(|skipped| skipped.as_bytes() == bytes)
                {
                    continue;
                }
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    children.push(entry.path());
                }
            }
            children.sort();
            stack.extend(children.into_iter().rev().map(|child| (child, depth + 1)));
        }
    }
    repositories.sort_by(|a, b| a.name.cmp(&b.name));
    Discovery {
        repositories,
        problems: problems.0,
        scanned_ms: unix_ms(SystemTime::now()),
        directories,
        truncated,
    }
}

/// Resolves a `.git` directory, or a `.git` file of a work tree or submodule,
/// to the common git directory.
fn repository(root: &Path, work_tree: &Path, dot_git: &Path) -> Result<Repository> {
    let git_dir = if fs::metadata(dot_git)?.is_dir() {
        dot_git.to_path_buf()
    } else {
        let mut text = String::new();
        fs::File::open(dot_git)?
            .take(4096)
            .read_to_string(&mut text)?;
        let target = text
            .trim()
            .strip_prefix("gitdir:")
            .context("broken .git file: no gitdir")?
            .trim();
        let target = work_tree.join(target);
        ensure!(
            target.is_dir(),
            "broken .git file: {} does not exist",
            target.display()
        );
        target
    };
    ensure!(
        git_dir.join("HEAD").is_file(),
        "not a git repository: {} has no HEAD",
        git_dir.display()
    );
    let common = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(text) => git_dir.join(text.trim()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => git_dir.clone(),
        Err(error) => return Err(error.into()),
    };
    let common = fs::canonicalize(&common)?;
    let main = common
        .parent()
        .filter(|main| common.file_name() == Some(".git".as_ref()) && main.starts_with(root));
    let path = main.unwrap_or(work_tree).to_path_buf();
    Ok(Repository {
        name: relative(root, &path),
        path,
        git_dir: common,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitKind {
    Commit,
    Merge,
    CherryPick,
    Revert,
    /// Applied from a patch with `git am`.
    Patch,
}

/// Where a commit's time comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitTiming {
    /// When the commit was made in this clone.
    Reflog,
    /// The commit's committer date, for repositories without reflogs.
    CommitterDate,
}

#[derive(Debug, Clone, Serialize)]
pub struct Commit {
    pub repository: String,
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    /// The branch it was committed on; null when detached or unknown.
    pub branch: Option<String>,
    pub time_ms: i64,
    pub timing: CommitTiming,
    pub kind: CommitKind,
    /// How often it was amended; `first_ms` is when the amended original was made.
    pub amends: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_ms: Option<i64>,
}

pub struct Reading {
    pub commits: Vec<Commit>,
    pub problems: Vec<RepositoryProblem>,
}

/// The user's commits made in `[start_ms, end_ms)`, ordered by time. A
/// repository that cannot be read is reported without failing the others;
/// `check` can stop the read between repositories.
pub fn read(
    discovery: &Discovery,
    sources: &GitSources,
    start_ms: i64,
    end_ms: i64,
    check: impl Fn() -> Result<()>,
) -> Result<Reading> {
    let deadline = Instant::now() + READ_BUDGET;
    let mut commits = Vec::new();
    let mut problems = Problems::default();
    for repository in &discovery.repositories {
        check()?;
        if Instant::now() >= deadline {
            problems.push(
                repository.name.clone(),
                format!(
                    "skipped: reading took longer than {} s",
                    READ_BUDGET.as_secs()
                ),
            );
            continue;
        }
        let reader = Reader {
            repository,
            sources,
            start_ms,
            end_ms,
            deadline,
        };
        match reader.commits() {
            Ok(found) => commits.extend(found),
            Err(error) => problems.push(repository.name.clone(), format!("{error:#}")),
        }
    }
    commits.sort_by(|a, b| {
        (a.time_ms, &a.repository, &a.hash).cmp(&(b.time_ms, &b.repository, &b.hash))
    });
    Ok(Reading {
        commits,
        problems: problems.0,
    })
}

fn modified(path: &Path) -> Option<i64> {
    fs::metadata(path).ok()?.modified().ok().map(unix_ms)
}

/// One counted reflog event.
struct Event {
    old: String,
    hash: String,
    time_ms: i64,
    email: String,
    kind: CommitKind,
    amend: bool,
    subject: String,
    branch: Option<String>,
}

/// Which reflog messages record a commit made here, by their action prefix.
fn classify(message: &str) -> Option<(CommitKind, bool)> {
    let (action, detail) = message.split_once(": ").unwrap_or((message, ""));
    let made_merge = detail.starts_with("Merge made by");
    Some(match action {
        "commit (amend)" => (CommitKind::Commit, true),
        "commit (merge)" => (CommitKind::Merge, false),
        "commit (cherry-pick)" | "cherry-pick" => (CommitKind::CherryPick, false),
        "revert" => (CommitKind::Revert, false),
        "am" => (CommitKind::Patch, false),
        _ if action == "commit" || action.starts_with("commit (") => (CommitKind::Commit, false),
        _ if (action.starts_with("merge ") || action == "pull" || action.starts_with("pull "))
            && !action.contains("--rebase")
            && made_merge =>
        {
            (CommitKind::Merge, false)
        }
        _ => return None,
    })
}

fn object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `<old> <new> <name> <<email>> <seconds> <zone>\t<message>`
fn parse_line(line: &str, branch: Option<&str>) -> Option<Event> {
    let (head, message) = line.split_once('\t')?;
    let (kind, amend) = classify(message)?;
    let mut fields = head.splitn(3, ' ');
    let old = fields.next()?;
    let hash = fields.next()?;
    let identity = fields.next()?;
    if !object_id(old) || !object_id(hash) || hash.bytes().all(|b| b == b'0') {
        return None;
    }
    let (email, seconds) = ident(identity)?;
    Some(Event {
        old: old.to_ascii_lowercase(),
        hash: hash.to_ascii_lowercase(),
        time_ms: seconds.checked_mul(1000)?,
        email,
        kind,
        amend,
        subject: message
            .split_once(": ")
            .map_or("", |(_, subject)| subject)
            .to_owned(),
        branch: branch.map(Into::into),
    })
}

/// The lowercased email and seconds of `Name <email> <seconds> <zone>`.
fn ident(text: &str) -> Option<(String, i64)> {
    let open = text.rfind('<')?;
    let close = open + text[open..].find('>')?;
    let seconds = text[close + 1..].split_whitespace().next()?.parse().ok()?;
    Some((text[open + 1..close].trim().to_lowercase(), seconds))
}

fn read_reflog(path: &Path, branch: Option<&str>, events: &mut Vec<Event>) -> Result<()> {
    let mut file =
        fs::File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let length = file.metadata()?.len();
    let mut bytes = Vec::new();
    if length > REFLOG_BYTES {
        file.seek(SeekFrom::Start(length - REFLOG_BYTES))?;
    }
    file.take(REFLOG_BYTES).read_to_end(&mut bytes)?;
    let mut text = String::from_utf8_lossy(&bytes);
    if length > REFLOG_BYTES {
        // Drop the partial first line of a tail.
        text = text
            .split_once('\n')
            .map_or("", |(_, rest)| rest)
            .to_owned()
            .into();
    }
    events.extend(text.lines().filter_map(|line| parse_line(line, branch)));
    Ok(())
}

/// Files below `directory`, with their names relative to it, modified since `since_ms`.
fn files_since(directory: &Path, since_ms: i64, prefix: &str, found: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        match entry.file_type() {
            Ok(kind) if kind.is_dir() && prefix.matches('/').count() < 8 => {
                files_since(&entry.path(), since_ms, &format!("{name}/"), found)
            }
            Ok(kind)
                if kind.is_file() && modified(&entry.path()).is_some_and(|m| m >= since_ms) =>
            {
                found.push((entry.path(), name))
            }
            _ => {}
        }
    }
}

struct Reader<'a> {
    repository: &'a Repository,
    sources: &'a GitSources,
    start_ms: i64,
    end_ms: i64,
    deadline: Instant,
}
impl Reader<'_> {
    fn git_dir(&self) -> &Path {
        &self.repository.git_dir
    }
    fn commits(&self) -> Result<Vec<Commit>> {
        let mut heads = vec![self.git_dir().join("logs/HEAD")];
        if let Ok(worktrees) = fs::read_dir(self.git_dir().join("worktrees")) {
            heads.extend(
                worktrees
                    .flatten()
                    .map(|entry| entry.path().join("logs/HEAD")),
            );
        }
        let heads: Vec<(PathBuf, i64)> = heads
            .into_iter()
            .filter_map(|path| modified(&path).map(|time| (path, time)))
            .collect();
        if heads.is_empty() {
            return self.without_reflog();
        }
        // A reflog last written before the range holds nothing in it.
        let mut events = Vec::new();
        for (path, _) in heads.iter().filter(|(_, time)| *time >= self.start_ms) {
            read_reflog(path, None, &mut events)?;
        }
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut branches = Vec::new();
        files_since(
            &self.git_dir().join("logs/refs/heads"),
            self.start_ms,
            "",
            &mut branches,
        );
        for (path, branch) in branches {
            read_reflog(&path, Some(&branch), &mut events)?;
        }
        let events = merge(events);
        let events: Vec<_> = events
            .into_iter()
            .filter(|(event, _, _)| (self.start_ms..self.end_ms).contains(&event.time_ms))
            .collect();
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let identities = self.identities()?;
        let hashes: Vec<&str> = events
            .iter()
            .map(|(event, _, _)| event.hash.as_str())
            .collect();
        let objects = self.objects(&hashes)?;
        Ok(events
            .into_iter()
            .filter_map(|(event, first_ms, amends)| {
                let object = objects.get(&event.hash);
                let author = object.map_or(&event.email, |object| &object.author);
                identities.contains(author).then(|| Commit {
                    repository: self.repository.name.clone(),
                    short_hash: event.hash[..7].into(),
                    subject: object
                        .map_or_else(|| truncate(&event.subject), |object| object.subject.clone()),
                    branch: event.branch,
                    time_ms: event.time_ms,
                    timing: CommitTiming::Reflog,
                    kind: event.kind,
                    amends,
                    first_ms: (amends > 0).then_some(first_ms),
                    hash: event.hash,
                })
            })
            .collect())
    }

    /// Without reflogs only committer dates remain, so commits fetched from
    /// elsewhere also count. Skipped unless a ref changed since the range
    /// began; results are reused until a ref changes.
    fn without_reflog(&self) -> Result<Vec<Commit>> {
        type Key = (PathBuf, i64, i64, i64, Vec<String>);
        static CACHE: OnceLock<Mutex<HashMap<Key, Vec<Commit>>>> = OnceLock::new();
        let mut refs = Vec::new();
        files_since(&self.git_dir().join("refs/heads"), i64::MIN, "", &mut refs);
        let changed = refs
            .iter()
            .map(|(path, _)| path.clone())
            .chain(["HEAD", "packed-refs"].map(|name| self.git_dir().join(name)))
            .filter_map(|path| modified(&path))
            .max()
            .unwrap_or(i64::MIN);
        if changed < self.start_ms {
            return Ok(Vec::new());
        }
        let identities = self.identities()?;
        let key = (
            self.git_dir().to_path_buf(),
            changed,
            self.start_ms,
            self.end_ms,
            identities.clone(),
        );
        let cache = CACHE.get_or_init(Default::default);
        if let Some(commits) = cache.lock().unwrap().get(&key) {
            return Ok(commits.clone());
        }
        let output = self.git(
            &[
                "log",
                "--all",
                &format!("--since=@{}", self.start_ms.div_euclid(1000)),
                &format!("--until=@{}", self.end_ms.div_euclid(1000) + 1),
                &format!("--max-count={FALLBACK_COMMITS}"),
                "--format=%H%x1f%ae%x1f%ct%x1f%P%x1f%s%x1e",
            ],
            None,
            OBJECT_BYTES,
        )?;
        ensure!(output.code == Some(0), "git log failed: {}", output.stderr);
        let commits = self.parse_log(&output.stdout, &identities);
        let mut cache = cache.lock().unwrap();
        if cache.len() >= 1024 {
            cache.clear();
        }
        cache.insert(key, commits.clone());
        Ok(commits)
    }
    fn parse_log(&self, output: &[u8], identities: &[String]) -> Vec<Commit> {
        String::from_utf8_lossy(output)
            .split('\x1e')
            .filter_map(|record| {
                let fields: Vec<&str> = record.trim_start_matches('\n').split('\x1f').collect();
                let [hash, email, seconds, parents, subject] = fields[..] else {
                    return None;
                };
                let time_ms = seconds.parse::<i64>().ok()? * 1000;
                (object_id(hash)
                    && identities.contains(&email.to_lowercase())
                    && (self.start_ms..self.end_ms).contains(&time_ms))
                .then(|| Commit {
                    repository: self.repository.name.clone(),
                    hash: hash.into(),
                    short_hash: hash[..7].into(),
                    subject: truncate(subject),
                    branch: None,
                    time_ms,
                    timing: CommitTiming::CommitterDate,
                    kind: if parents.split_whitespace().count() > 1 {
                        CommitKind::Merge
                    } else {
                        CommitKind::Commit
                    },
                    amends: 0,
                    first_ms: None,
                })
            })
            .collect()
    }

    /// The repository's effective `user.email`, including conditional
    /// includes, plus the configured authors.
    fn identities(&self) -> Result<Vec<String>> {
        type Cached = HashMap<PathBuf, (Instant, Option<String>)>;
        static CACHE: OnceLock<Mutex<Cached>> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        let cached = cache
            .lock()
            .unwrap()
            .get(self.git_dir())
            .filter(|(at, _)| at.elapsed() < REFRESH)
            .map(|(_, email)| email.clone());
        let email = match cached {
            Some(email) => email,
            None => {
                let output = self.git(&["config", "--get", "user.email"], None, 4096)?;
                let email = match output.code {
                    Some(0) => Some(
                        String::from_utf8_lossy(&output.stdout)
                            .trim()
                            .to_lowercase(),
                    ),
                    Some(1) => None,
                    _ => bail!("git config failed: {}", output.stderr),
                };
                let mut cache = cache.lock().unwrap();
                if cache.len() >= 4096 {
                    cache.clear();
                }
                cache.insert(self.git_dir().into(), (Instant::now(), email.clone()));
                email
            }
        };
        let mut identities = self.sources.authors.clone();
        identities.extend(email.filter(|email| !email.is_empty()));
        ensure!(
            !identities.is_empty(),
            "no user.email is configured for this repository; add yours to [git] authors"
        );
        Ok(identities)
    }

    /// Author and subject of commit objects, which never change once made.
    /// Objects that no longer exist are absent.
    fn objects(&self, hashes: &[&str]) -> Result<HashMap<String, Arc<CommitObject>>> {
        type Cached = HashMap<(PathBuf, String), Arc<CommitObject>>;
        static CACHE: OnceLock<Mutex<Cached>> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        let mut found = HashMap::new();
        let mut missing = String::new();
        {
            let cache = cache.lock().unwrap();
            for hash in hashes {
                match cache.get(&(self.git_dir().to_path_buf(), (*hash).to_owned())) {
                    Some(object) => {
                        found.insert((*hash).to_owned(), object.clone());
                    }
                    None => {
                        missing.push_str(hash);
                        missing.push('\n');
                    }
                }
            }
        }
        if missing.is_empty() {
            return Ok(found);
        }
        let output = self.git(
            &["cat-file", "--batch"],
            Some(missing.into_bytes()),
            OBJECT_BYTES,
        )?;
        ensure!(
            output.code == Some(0),
            "git cat-file failed: {}",
            output.stderr
        );
        let mut cache = cache.lock().unwrap();
        if cache.len() >= 16_384 {
            cache.clear();
        }
        for (hash, object) in parse_batch(&output.stdout) {
            let object = Arc::new(object);
            cache.insert((self.git_dir().to_path_buf(), hash.clone()), object.clone());
            found.insert(hash, object);
        }
        Ok(found)
    }

    fn git(&self, arguments: &[&str], input: Option<Vec<u8>>, limit: usize) -> Result<Output> {
        git(self.git_dir(), arguments, input, limit, self.deadline)
    }
}

/// Collapses events to one per commit, earliest first, taking the branch from
/// a branch reflog, and folds amends into the commit they replace. Returns
/// each remaining event with when its first version was made and its amends.
fn merge(mut events: Vec<Event>) -> Vec<(Event, i64, u32)> {
    // Stable: HEAD reflogs, in order, come before branch reflogs.
    events.sort_by_key(|event| event.time_ms);
    let mut commits: Vec<Option<(Event, i64, u32)>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for event in events {
        if let Some(&known) = index.get(&event.hash) {
            if let Some((commit, _, _)) = commits[known].as_mut()
                && commit.branch.is_none()
            {
                commit.branch = event.branch;
            }
            continue;
        }
        let mut entry = (event, 0, 0);
        entry.1 = entry.0.time_ms;
        if entry.0.amend
            && let Some(&amended) = index.get(&entry.0.old)
            && let Some((previous, first_ms, amends)) = commits[amended].take()
        {
            entry.1 = first_ms;
            entry.2 = amends + 1;
            if entry.0.branch.is_none() {
                entry.0.branch = previous.branch;
            }
        }
        index.insert(entry.0.hash.clone(), commits.len());
        commits.push(Some(entry));
    }
    commits.into_iter().flatten().collect()
}

#[derive(Debug, PartialEq)]
struct CommitObject {
    author: String,
    subject: String,
}

fn truncate(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(SUBJECT_CHARS) {
        Some((index, _)) => format!("{}…", &text[..index]),
        None => text.into(),
    }
}

/// Commits from `git cat-file --batch` output; missing objects are skipped.
fn parse_batch(mut output: &[u8]) -> Vec<(String, CommitObject)> {
    let mut objects = Vec::new();
    while let Some(end) = output.iter().position(|&b| b == b'\n') {
        let header = String::from_utf8_lossy(&output[..end]).into_owned();
        output = &output[end + 1..];
        let fields: Vec<&str> = header.split(' ').collect();
        let [hash, kind, size] = fields[..] else {
            continue;
        };
        let Ok(size) = size.parse::<usize>() else {
            break;
        };
        let Some(body) = output.get(..size) else {
            break;
        };
        output = output.get(size + 1..).unwrap_or_default();
        if kind != "commit" {
            continue;
        }
        let text = String::from_utf8_lossy(body);
        let (headers, message) = text.split_once("\n\n").unwrap_or((&text, ""));
        let Some(author) = headers
            .lines()
            .find_map(|line| line.strip_prefix("author "))
            .and_then(ident)
        else {
            continue;
        };
        let paragraph = message.trim_start().split("\n\n").next().unwrap_or("");
        let subject = paragraph
            .lines()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        objects.push((
            hash.to_owned(),
            CommitObject {
                author: author.0,
                subject: truncate(&subject),
            },
        ));
    }
    objects
}

struct Output {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

/// Runs a read-only git command against `git_dir` without optional locks,
/// prompts or a pager, bounded in time and output.
fn git(
    git_dir: &Path,
    arguments: &[&str],
    input: Option<Vec<u8>>,
    limit: usize,
    deadline: Instant,
) -> Result<Output> {
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("--no-pager")
        .arg("--git-dir")
        .arg(git_dir)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
    ] {
        command.env_remove(variable);
    }
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("cannot run git")?;
    let writer = input.map(|bytes| {
        let mut stdin = child.stdin.take().unwrap();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        })
    });
    let overflow = Arc::new(AtomicBool::new(false));
    let mut stdout = child.stdout.take().unwrap();
    let reader = {
        let overflow = overflow.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = Read::by_ref(&mut stdout)
                .take(limit as u64 + 1)
                .read_to_end(&mut bytes);
            if bytes.len() > limit {
                overflow.store(true, Ordering::Relaxed);
            }
            bytes
        })
    };
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = Read::by_ref(&mut stderr).take(4096).read_to_end(&mut bytes);
        bytes
    });
    let timeout = deadline.min(Instant::now() + COMMAND_TIMEOUT);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if overflow.load(Ordering::Relaxed) || Instant::now() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let stdout = reader.join().unwrap_or_default();
    let stderr = errors.join().unwrap_or_default();
    ensure!(
        !overflow.load(Ordering::Relaxed),
        "git output exceeded {limit} bytes"
    );
    let Some(status) = status else {
        bail!("git took longer than {} s", COMMAND_TIMEOUT.as_secs());
    };
    Ok(Output {
        code: status.code(),
        stdout,
        stderr: String::from_utf8_lossy(&stderr)
            .lines()
            .next()
            .unwrap_or("")
            .to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counted_reflog_events_are_commits_made_here() {
        let counted = [
            ("commit (initial): first", CommitKind::Commit, false),
            ("commit: second", CommitKind::Commit, false),
            ("commit (amend): second, amended", CommitKind::Commit, true),
            ("commit (merge): resolved", CommitKind::Merge, false),
            (
                "commit (cherry-pick): resolved",
                CommitKind::CherryPick,
                false,
            ),
            ("cherry-pick: third", CommitKind::CherryPick, false),
            ("revert: Revert \"third\"", CommitKind::Revert, false),
            ("am: patch", CommitKind::Patch, false),
            (
                "merge feature: Merge made by the 'ort' strategy.",
                CommitKind::Merge,
                false,
            ),
            (
                "pull: Merge made by the 'ort' strategy.",
                CommitKind::Merge,
                false,
            ),
        ];
        for (message, kind, amend) in counted {
            assert_eq!(classify(message), Some((kind, amend)), "{message}");
        }
        for message in [
            "checkout: moving from main to feature",
            "rebase (pick): third",
            "rebase (finish): returning to refs/heads/feature",
            "pull --rebase (pick): third",
            "reset: moving to HEAD~1",
            "merge feature: Fast-forward",
            "pull: Fast-forward",
            "branch: Created from HEAD",
            "clone: from git@example.com:repo.git",
            "",
        ] {
            assert_eq!(classify(message), None, "{message}");
        }
    }

    #[test]
    fn reflog_lines_and_batch_objects_parse() {
        let old = "0".repeat(40);
        let new = "a".repeat(40);
        let line = format!(
            "{old} {new} A Person <Me@Example.com> 1700000000 +0100\tcommit (initial): first: line"
        );
        let event = parse_line(&line, Some("main")).unwrap();
        assert_eq!(event.email, "me@example.com");
        assert_eq!(event.time_ms, 1_700_000_000_000);
        assert_eq!(event.subject, "first: line");
        assert_eq!(event.branch.as_deref(), Some("main"));
        assert!(parse_line(&format!("{old} {new} A <a@b> 1 +0000"), None).is_none());
        assert!(parse_line(&format!("{new} {old} A <a@b> 1 +0000\tcommit: x"), None).is_none());

        let body = "tree t\nparent p\nauthor A <a@b.c> 1 +0000\ncommitter C <c@d> 2 +0000\n\n  Subject\nwraps\n\nBody\n";
        let output = format!(
            "{new} commit {}\n{body}\n{} missing\n{old} blob 3\nabc\n",
            body.len(),
            "b".repeat(40)
        );
        assert_eq!(
            parse_batch(output.as_bytes()),
            [(
                new.clone(),
                CommitObject {
                    author: "a@b.c".into(),
                    subject: "Subject wraps".into()
                }
            )]
        );
    }

    #[test]
    fn amends_fold_into_one_commit_and_duplicates_count_once() {
        let event = |old: char, hash: char, time_ms, amend, branch: Option<&str>| Event {
            old: old.to_string().repeat(40),
            hash: hash.to_string().repeat(40),
            time_ms,
            email: "me@x".into(),
            kind: CommitKind::Commit,
            amend,
            subject: String::new(),
            branch: branch.map(Into::into),
        };
        let merged = merge(vec![
            event('0', 'a', 10, false, None),
            event('a', 'b', 20, true, None),
            event('b', 'c', 30, true, None),
            event('c', 'd', 40, false, None),
            event('0', 'a', 10, false, Some("main")),
            event('c', 'd', 40, false, Some("main")),
        ]);
        let summary: Vec<_> = merged
            .iter()
            .map(|(event, first, amends)| {
                (
                    &event.hash[..1],
                    event.time_ms,
                    *first,
                    *amends,
                    event.branch.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("c", 30, 10, 2, Some("main")),
                ("d", 40, 40, 0, Some("main"))
            ]
        );
    }

    #[test]
    fn sources_expand_home_and_validate() {
        let parse = |text: &str| -> Result<GitSources> {
            GitSources::new(toml::from_str(text)?, Some(Path::new("/home/me")))
        };
        let sources =
            parse("paths = ['~/work', '~', '/srv/code']\nauthors = [' Me@Example.com ']").unwrap();
        assert_eq!(
            sources.roots,
            [
                PathBuf::from("/home/me/work"),
                PathBuf::from("/home/me"),
                PathBuf::from("/srv/code")
            ]
        );
        assert_eq!(sources.authors, ["me@example.com"]);
        assert_eq!(sources.depth, DEFAULT_DEPTH);
        for invalid in [
            "paths = ['work']",
            "paths = ['~other/work']",
            "authors = ['me']",
            "depth = 0",
            "path = ['~/work']",
        ] {
            assert!(parse(invalid).is_err(), "{invalid}");
        }
    }
}
