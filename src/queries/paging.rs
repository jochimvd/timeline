//! Opaque continuation cursors and byte-bounded pages. Cursors pin the query
//! and append watermarks; retention epochs invalidate them explicitly.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Cursor {
    pub query: String,
    pub observations: i64,
    pub screenshots: i64,
    pub epoch: i64,
    pub offset: usize,
}

impl Queries {
    pub(super) fn cursor(&self, query: String, page: &PageRequest) -> Result<Cursor> {
        if !(1..=500).contains(&page.limit.unwrap_or(100)) {
            bail!(QueryError::InvalidRequest);
        }
        let connection = &self.history.connection;
        let epoch = connection
            .query_row(
                "SELECT value FROM state WHERE key='retention_epoch'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if let Some(token) = &page.cursor {
            if token.len() > 4096 {
                bail!(QueryError::InvalidRequest);
            }
            let bytes = decode_cursor(token)?;
            let cursor: Cursor =
                serde_json::from_slice(&bytes).map_err(|_| QueryError::InvalidRequest)?;
            if cursor.query != query {
                bail!(QueryError::InvalidRequest);
            }
            if cursor.epoch != epoch {
                bail!(QueryError::CursorInvalidated);
            }
            return Ok(cursor);
        }
        Ok(Cursor {
            query,
            epoch,
            offset: 0,
            observations: connection.query_row(
                "SELECT coalesce(max(id),0) FROM observations",
                [],
                |r| r.get(0),
            )?,
            screenshots: connection.query_row(
                "SELECT coalesce(max(id),0) FROM screenshots",
                [],
                |r| r.get(0),
            )?,
        })
    }
    /// Process caches are keyed by the database file, never shared across histories.
    pub(super) fn database_key(&self) -> Result<String> {
        let path = self
            .history
            .directory
            .join("history.sqlite3")
            .canonicalize()?;
        let metadata = std::fs::metadata(&path)?;
        Ok(format!(
            "{}:{}:{}",
            path.display(),
            metadata.dev(),
            metadata.ino()
        ))
    }
}

pub(super) struct Pager<T> {
    cursor: Cursor,
    limit: usize,
    pub seen: usize,
    bytes: usize,
    items: Vec<T>,
    pub more: bool,
    truncated: bool,
}
impl<T: Serialize> Pager<T> {
    pub fn new(cursor: Cursor, request: &PageRequest) -> Self {
        Self {
            cursor,
            limit: request.limit.unwrap_or(100),
            seen: 0,
            bytes: 0,
            items: vec![],
            more: false,
            truncated: false,
        }
    }
    pub fn push(&mut self, item: T) -> Result<()> {
        let index = self.seen;
        self.seen += 1;
        if index < self.cursor.offset {
            return Ok(());
        }
        if self.more || self.items.len() >= self.limit {
            self.more = true;
            return Ok(());
        }
        let size = serde_json::to_vec(&item)?.len();
        if size > PAGE_BYTES {
            bail!(QueryError::LimitExceeded);
        }
        if self.bytes + size > PAGE_BYTES {
            self.more = true;
            self.truncated = true;
            return Ok(());
        }
        self.bytes += size;
        self.items.push(item);
        Ok(())
    }
    pub fn finish(mut self) -> Result<Page<T>> {
        self.cursor.offset += self.items.len();
        let next_cursor = self.more.then(|| encode_cursor(&self.cursor)).transpose()?;
        Ok(Page {
            items: self.items,
            next_cursor,
            truncated: self.truncated,
        })
    }
}

pub(super) fn encode_cursor(cursor: &Cursor) -> Result<String> {
    Ok(serde_json::to_vec(cursor)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
pub(super) fn decode_cursor(token: &str) -> Result<Vec<u8>> {
    if !token.is_ascii() || !token.len().is_multiple_of(2) {
        bail!(QueryError::InvalidRequest);
    }
    token
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|_| QueryError::InvalidRequest)?;
            u8::from_str_radix(text, 16).map_err(|_| QueryError::InvalidRequest.into())
        })
        .collect()
}
