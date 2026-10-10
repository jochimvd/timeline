//! Exact totals plus original or display-resolution coverage rows. Rows are
//! emitted as applications, exact titles, then focus and image coverage.
use super::*;

// Immutable serialized rows keep pagination cache accounting independent of
// Rust enum/string allocation overhead. One result is capped at 8 MiB and the
// process cache at 16 MiB; two admitted readers can retain two evicted results.
const OVERVIEW_CACHE_BYTES: usize = 16 * 1024 * 1024;
pub(super) struct CachedOverview {
    key: String,
    pub totals: Totals,
    pub rows: Vec<Box<[u8]>>,
    bytes: usize,
}
#[derive(Default)]
struct OverviewCache {
    entries: VecDeque<Arc<CachedOverview>>,
    bytes: usize,
}
static OVERVIEWS: OnceLock<Mutex<OverviewCache>> = OnceLock::new();
pub(super) fn cached_overview(key: &str) -> Option<Arc<CachedOverview>> {
    let mut cache = OVERVIEWS.get_or_init(Default::default).lock().unwrap();
    let index = cache.entries.iter().position(|entry| entry.key == key)?;
    let entry = cache.entries.remove(index).unwrap();
    cache.entries.push_back(entry.clone());
    Some(entry)
}
fn remember_overview(entry: CachedOverview) {
    let mut cache = OVERVIEWS.get_or_init(Default::default).lock().unwrap();
    if cache.entries.iter().any(|old| old.key == entry.key) {
        return;
    }
    while cache.bytes + entry.bytes > OVERVIEW_CACHE_BYTES {
        let old = cache.entries.pop_front().unwrap();
        cache.bytes -= old.bytes;
    }
    cache.bytes += entry.bytes;
    cache.entries.push_back(Arc::new(entry));
}
struct OverviewBuilder {
    rows: Option<Vec<Box<[u8]>>>,
    bytes: usize,
}
impl OverviewBuilder {
    fn new() -> Self {
        Self {
            rows: Some(Vec::new()),
            bytes: 16 * 1024,
        }
    }
    fn push(&mut self, item: &Breakdown) -> Result<()> {
        if let Some(rows) = &mut self.rows {
            let encoded = serde_json::to_vec(item)?.into_boxed_slice();
            // Account for capacity growth in the vector and allocator overhead.
            self.bytes += encoded.len() + 64;
            if self.bytes > BREAKDOWN_BYTES {
                self.rows = None;
            } else {
                rows.push(encoded);
            }
        }
        Ok(())
    }
}

/// Exact focus totals and per-application exact-title breakdown.
#[derive(Default)]
struct Summary {
    totals: Totals,
    titles: BTreeMap<String, BTreeMap<String, i64>>,
    bytes: usize,
}
impl Summary {
    fn add(&mut self, coverage: &Coverage) -> Result<()> {
        let elapsed = coverage.elapsed_ms.unwrap_or(0);
        match &coverage.focus {
            FocusCoverage::Focus { window, idle } => {
                self.totals.focus_ms += elapsed;
                if *idle {
                    self.totals.idle_ms += elapsed;
                }
                // Look up by reference; only a new key allocates.
                let titles = match self.titles.get_mut(&window.application) {
                    Some(titles) => titles,
                    None => self.titles.entry(window.application.clone()).or_default(),
                };
                match titles.get_mut(&window.title) {
                    Some(total) => *total += elapsed,
                    None => {
                        self.bytes += window.application.len() * 2 + window.title.len() + 128;
                        if self.bytes > BREAKDOWN_BYTES {
                            bail!(QueryError::LimitExceeded);
                        }
                        titles.insert(window.title.clone(), elapsed);
                    }
                }
            }
            FocusCoverage::NoFocus { idle } => {
                self.totals.no_focus_ms += elapsed;
                if *idle {
                    self.totals.idle_ms += elapsed;
                }
            }
            _ => (),
        }
        Ok(())
    }
    /// Application rows, then exact-title rows, each in name order.
    fn rows(self) -> impl Iterator<Item = Breakdown> {
        let focus_ms = self.totals.focus_ms;
        let percent = move |elapsed| {
            if focus_ms == 0 {
                0.0
            } else {
                elapsed as f64 * 100.0 / focus_ms as f64
            }
        };
        let applications: Vec<_> = self
            .titles
            .iter()
            .map(|(application, titles)| {
                let elapsed_ms = titles.values().sum();
                Breakdown::Application {
                    application: application.clone(),
                    elapsed_ms,
                    percent_of_focus: percent(elapsed_ms),
                }
            })
            .collect();
        let titles = self
            .titles
            .into_iter()
            .flat_map(move |(application, titles)| {
                titles
                    .into_iter()
                    .map(move |(title, elapsed_ms)| Breakdown::ExactTitle {
                        application: application.clone(),
                        title,
                        elapsed_ms,
                        percent_of_focus: percent(elapsed_ms),
                    })
            });
        applications.into_iter().chain(titles)
    }
}

/// Original focus rows wait for the totals in the same pass. Past the breakdown
/// budget they are dropped and recomputed by a second pass instead.
enum Held {
    Rows(Vec<Coverage>, usize),
    Overflowed,
}
impl Held {
    fn push(&mut self, coverage: Coverage) {
        if let Held::Rows(rows, bytes) = self {
            *bytes += std::mem::size_of::<Coverage>()
                + match &coverage.focus {
                    FocusCoverage::Focus { window, .. } => {
                        window.address.len() + window.application.len() + window.title.len()
                    }
                    FocusCoverage::Gap { reason } => reason.len(),
                    _ => 0,
                };
            if *bytes > BREAKDOWN_BYTES {
                *self = Held::Overflowed;
            } else {
                rows.push(coverage);
            }
        }
    }
}

impl Queries {
    pub(super) fn overview_key(&self, cursor: &Cursor) -> Result<String> {
        Ok(format!(
            "{}:{}:{}:{}:{}",
            self.database_key()?,
            cursor.query,
            cursor.observations,
            cursor.screenshots,
            cursor.epoch
        ))
    }
    pub fn overview(
        &self,
        client: Client,
        selection: Selection,
        page: PageRequest,
    ) -> Result<Overview> {
        self.overview_display(client, selection, page, None)
    }
    /// Optional viewport budget affects display coverage only, never exact totals or evidence.
    pub fn overview_display(
        &self,
        client: Client,
        selection: Selection,
        page: PageRequest,
        points: Option<usize>,
    ) -> Result<Overview> {
        let resolved = self.resolve(selection)?;
        let display = points
            .map(|points| display::Display::new(resolved.range, points))
            .transpose()?
            .filter(display::Display::coarse);
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.cursor(
            format!(
                "overview:{:?}:{}:{points:?}",
                resolved.range, resolved.timezone
            ),
            &page,
        )?;
        let key = self.overview_key(&cursor)?;
        let revision = encode_cursor(&Cursor {
            offset: 0,
            ..cursor.clone()
        })?;
        let mut output = Pager::new(cursor.clone(), &page);
        if let Some(cached) = cached_overview(&key) {
            // Skip directly to the requested page; Pager still enforces payload
            // limits, continuation offsets and identical truncation semantics.
            output.seen = cursor.offset;
            for row in cached.rows.iter().skip(cursor.offset) {
                output.push(serde_json::from_slice(row)?)?;
                if output.more {
                    break;
                }
            }
            return Ok(Overview {
                revision,
                resolved,
                totals: cached.totals.clone(),
                breakdown: output.finish()?,
            });
        }
        let mut cache = OverviewBuilder::new();
        let totals = self.overview_rows(&cursor, resolved.range, display, |item| {
            cache.push(&item)?;
            output.push(item)
        })?;
        if let Some(rows) = cache.rows {
            remember_overview(CachedOverview {
                key,
                totals: totals.clone(),
                rows,
                bytes: cache.bytes,
            });
        }
        Ok(Overview {
            revision,
            resolved,
            totals,
            breakdown: output.finish()?,
        })
    }
    /// One observation pass for totals, focus rows and image context, then one
    /// screenshot pass; a second observation pass only if held rows overflow.
    fn overview_rows(
        &self,
        cursor: &Cursor,
        range: Range,
        mut display: Option<display::Display>,
        mut emit: impl FnMut(Breakdown) -> Result<()>,
    ) -> Result<Totals> {
        let plan = self.plan(cursor, range, Scope::Evidence)?;
        let mut summary = Summary::default();
        let mut held = Held::Rows(Vec::new(), 0);
        let mut context = ImageContext::default();
        self.coverage(cursor, range, &plan, Some(&mut context), |coverage| {
            summary.add(&coverage)?;
            match &mut display {
                Some(display) => display.focus(coverage)?,
                None => held.push(coverage),
            }
            Ok(())
        })?;
        let totals = summary.totals.clone();
        for row in summary.rows() {
            emit(row)?;
        }
        if let Some(mut display) = display {
            self.image_coverage(cursor, range, None, &plan, &context, |coverage| {
                display.image(coverage)
            })?;
            for row in display.rows() {
                emit(row)?;
            }
            return Ok(totals);
        }
        match held {
            Held::Rows(rows, _) => {
                for coverage in rows {
                    emit(Breakdown::FocusCoverage(coverage))?;
                }
            }
            Held::Overflowed => self.coverage(cursor, range, &plan, None, |coverage| {
                emit(Breakdown::FocusCoverage(coverage))
            })?,
        }
        self.image_coverage(cursor, range, None, &plan, &context, |coverage| {
            emit(Breakdown::ImageCoverage(coverage))
        })?;
        Ok(totals)
    }
}
