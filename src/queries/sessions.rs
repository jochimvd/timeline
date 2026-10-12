//! Sessions: the focus stretches of a range in time order, each with the
//! windows it spent its own time in. A chronological account of observed focus
//! for reconstructing a day — not task attribution, and not task time.
use super::*;
use anyhow::ensure;

/// Windows listed per session, most active first.
const SESSION_WINDOWS: usize = 3;
/// Sessions cover at most a week of activity days, plus a daylight-saving hour.
const MAX_SESSION_RANGE_MS: i64 = 7 * 24 * HOUR + HOUR;

/// A window a session spent active focus in.
#[derive(Debug, Clone, Serialize)]
pub struct SessionWindow {
    pub application: String,
    /// Cleaned window title.
    pub title: String,
    pub active_ms: i64,
}
/// One focus stretch with its most active windows on its own subject.
#[derive(Debug, Clone, Serialize)]
pub struct FocusSession {
    #[serde(flatten)]
    pub stretch: FocusStretch,
    pub windows: Vec<SessionWindow>,
}
#[derive(Debug, Serialize)]
pub struct SessionList {
    pub resolved: ResolvedRange,
    pub sessions: Vec<FocusSession>,
}

/// Active focus on one window, in scan order.
struct Piece {
    start_ms: i64,
    window: usize,
    category: Option<usize>,
    elapsed_ms: i64,
}

impl Queries {
    /// Focus stretches of a selection up to seven activity days, in time order,
    /// from one coverage pass. They match the stretches counted by the overview
    /// metrics of the same range.
    pub fn sessions(&self, client: Client, selection: Selection) -> Result<SessionList> {
        let resolved = self.resolve(selection)?;
        let range = resolved.range;
        ensure!(
            range.end_ms - range.start_ms <= MAX_SESSION_RANGE_MS,
            QueryError::LimitExceeded
        );
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.cursor("sessions".into(), &PageRequest::default())?;
        let plan = self.plan(&cursor, range, Scope::Evidence)?;
        let mut metrics = self.metrics(range).keeping_stretches();
        // Windows and categories are interned; the byte budget bounds both.
        let mut windows: Vec<(String, String)> = Vec::new();
        let mut window_ids: HashMap<(String, String), usize> = HashMap::new();
        let mut categories: Vec<String> = Vec::new();
        let mut category_ids: HashMap<String, usize> = HashMap::new();
        let mut pieces: Vec<Piece> = Vec::new();
        let mut bytes = 0;
        self.coverage(&cursor, range, &plan, None, |coverage| {
            metrics.add(&coverage);
            let (
                FocusCoverage::Focus {
                    window,
                    idle: false,
                    category,
                },
                Some(elapsed_ms),
            ) = (&coverage.focus, coverage.elapsed_ms)
            else {
                return Ok(());
            };
            let key = (window.application.clone(), window.title.clone());
            let window = match window_ids.get(&key) {
                Some(&id) => id,
                None => {
                    bytes += key.0.len() + key.1.len() + 64;
                    windows.push(key.clone());
                    window_ids.insert(key, windows.len() - 1);
                    windows.len() - 1
                }
            };
            let category = category.as_ref().map(|name| match category_ids.get(name) {
                Some(&id) => id,
                None => {
                    bytes += name.len() + 64;
                    categories.push(name.clone());
                    category_ids.insert(name.clone(), categories.len() - 1);
                    categories.len() - 1
                }
            });
            bytes += 32;
            if bytes > BREAKDOWN_BYTES {
                bail!(QueryError::LimitExceeded);
            }
            pieces.push(Piece {
                start_ms: coverage.range.start_ms,
                window,
                category,
                elapsed_ms,
            });
            Ok(())
        })?;
        let (_, stretches) = metrics.finish_with_stretches();
        // Stretches and pieces are both in time order: one merge pass.
        let mut next = 0;
        let sessions = stretches
            .into_iter()
            .map(|stretch| {
                while next < pieces.len() && pieces[next].start_ms < stretch.range.start_ms {
                    next += 1;
                }
                let mut active: HashMap<usize, i64> = HashMap::new();
                while next < pieces.len() && pieces[next].start_ms < stretch.range.end_ms {
                    let piece = &pieces[next];
                    let own = match &stretch.subject {
                        FocusSubject::Category { name } => {
                            piece.category.is_some_and(|id| categories[id] == *name)
                        }
                        FocusSubject::Application { name } => {
                            piece.category.is_none() && windows[piece.window].0 == *name
                        }
                    };
                    if own {
                        *active.entry(piece.window).or_default() += piece.elapsed_ms;
                    }
                    next += 1;
                }
                let mut ranked: Vec<_> = active.into_iter().collect();
                ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| windows[a.0].cmp(&windows[b.0])));
                FocusSession {
                    windows: ranked
                        .into_iter()
                        .take(SESSION_WINDOWS)
                        .map(|(id, active_ms)| SessionWindow {
                            application: windows[id].0.clone(),
                            title: windows[id].1.clone(),
                            active_ms,
                        })
                        .collect(),
                    stretch,
                }
            })
            .collect();
        Ok(SessionList { resolved, sessions })
    }
}
