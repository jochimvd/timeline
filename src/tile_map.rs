//! Schema 5 maps: canonical positive unsigned LEB128 region identifiers.
use anyhow::{Result, ensure};
use rusqlite::{Connection, params};
pub(crate) const MAX_IDS: usize = 640;
pub(crate) const MAX_BYTES: usize = MAX_IDS * 9;

pub(crate) fn push(output: &mut Vec<u8>, id: i64) -> Result<()> {
    ensure!(id > 0, "invalid tile id");
    let mut value = id as u64;
    while value >= 128 {
        output.push(value as u8 | 128);
        value >>= 7;
    }
    output.push(value as u8);
    Ok(())
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Vec<i64>> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_BYTES,
        "invalid image map size"
    );
    let mut ids = Vec::new();
    let (mut value, mut shift) = (0u64, 0u32);
    for &byte in bytes {
        ensure!(shift <= 56, "tile id overflow");
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            ensure!(
                value > 0 && (shift == 0 || byte != 0),
                "invalid tile id encoding"
            );
            ids.push(value as i64);
            ensure!(ids.len() <= MAX_IDS, "too many tiles");
            value = 0;
            shift = 0;
        } else {
            shift += 7;
        }
    }
    ensure!(shift == 0, "truncated tile id");
    Ok(ids)
}
/// One atomic metadata-only conversion; packs and reference counts are untouched.
pub(crate) fn upgrade(db: &Connection) -> Result<()> {
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 4 {
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    let mut after = 0i64;
    loop {
        let rows = {
            let mut stmt =
                tx.prepare("SELECT rowid,map FROM images WHERE rowid>?1 ORDER BY rowid LIMIT 128")?;
            stmt.query_map([after], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if rows.is_empty() {
            break;
        }
        for (rowid, old) in rows {
            ensure!(
                !old.is_empty() && old.len().is_multiple_of(8) && old.len() / 8 <= MAX_IDS,
                "invalid legacy tile map"
            );
            let mut compact = Vec::new();
            for bytes in old.chunks_exact(8) {
                push(&mut compact, i64::from_le_bytes(bytes.try_into()?))?;
            }
            tx.execute(
                "UPDATE images SET map=?2 WHERE rowid=?1",
                params![rowid, compact],
            )?;
            after = rowid;
        }
    }
    tx.execute_batch("PRAGMA user_version=5")?;
    tx.commit()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_and_corruption() {
        let ids = [1, 127, 128, 16383, 16384, 1 << 21, 1 << 35, i64::MAX];
        let mut bytes = Vec::new();
        for id in ids {
            push(&mut bytes, id).unwrap();
        }
        assert_eq!(decode(&bytes).unwrap(), ids);
        for bad in [
            vec![],
            vec![0],
            vec![128],
            vec![129, 0],
            vec![255; 10],
            vec![1; MAX_IDS + 1],
        ] {
            assert!(decode(&bad).is_err());
        }
    }
    #[test]
    fn migration_is_atomic_and_preserves_ids() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE images(map BLOB); PRAGMA user_version=4;")
            .unwrap();
        let old = [1i64.to_le_bytes(), 98765i64.to_le_bytes()].concat();
        db.execute("INSERT INTO images VALUES(?1)", [&old]).unwrap();
        db.execute("INSERT INTO images VALUES(x'00')", []).unwrap();
        assert!(upgrade(&db).is_err());
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            4
        );
        assert_eq!(
            db.query_row("SELECT map FROM images WHERE rowid=1", [], |r| r
                .get::<_, Vec<u8>>(0))
                .unwrap(),
            old
        );
        db.execute("DELETE FROM images WHERE rowid=2", []).unwrap();
        upgrade(&db).unwrap();
        let compact = db
            .query_row("SELECT map FROM images", [], |r| r.get::<_, Vec<u8>>(0))
            .unwrap();
        assert_eq!(decode(&compact).unwrap(), [1, 98765]);
        assert_eq!(compact.len(), 4);
        upgrade(&db).unwrap();
    }
}
