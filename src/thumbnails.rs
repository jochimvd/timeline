//! Encoded preview LRU: bounded RAM only, never persistent image files.
use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
};
const BYTE_LIMIT: usize = 16 * 1024 * 1024;
const ENTRY_LIMIT: usize = 128;
#[derive(Default)]
struct Cache {
    entries: VecDeque<(String, Vec<u8>)>,
    bytes: usize,
}
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}
pub(crate) fn get(key: &str) -> Option<Vec<u8>> {
    let mut cache = cache().lock().unwrap();
    let index = cache.entries.iter().position(|(k, _)| k == key)?;
    let entry = cache.entries.remove(index)?;
    let bytes = entry.1.clone();
    cache.entries.push_back(entry);
    Some(bytes)
}
pub(crate) fn insert(key: String, bytes: &[u8]) {
    let mut cache = cache().lock().unwrap();
    if bytes.len() > BYTE_LIMIT || cache.entries.iter().any(|(k, _)| k == &key) {
        return;
    }
    while cache.bytes + bytes.len() > BYTE_LIMIT || cache.entries.len() >= ENTRY_LIMIT {
        if let Some((_, old)) = cache.entries.pop_front() {
            cache.bytes -= old.len();
        }
    }
    cache.bytes += bytes.len();
    cache.entries.push_back((key, bytes.to_vec()));
}
pub(crate) fn usage() -> usize {
    cache().lock().unwrap().bytes
}
