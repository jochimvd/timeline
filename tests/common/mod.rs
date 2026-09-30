#![allow(dead_code)]

pub fn webp(tag: &[u8]) -> Vec<u8> {
    let hash = blake3::hash(tag);
    let h = hash.as_bytes();
    timeline::codec::encode_bgra(&[h[0], h[1], h[2], 255, h[3], h[4], h[5], 255], 2, 1, true)
        .unwrap()
}

pub fn pack_files(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(directory.join("tiles"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "bin"))
        .collect()
}

pub fn remove_packs(directory: &std::path::Path) {
    for path in pack_files(directory) {
        std::fs::remove_file(path).unwrap();
    }
}

pub fn payload_files(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(directory.join("tiles"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "bin" || e == "part"))
        .collect()
}
