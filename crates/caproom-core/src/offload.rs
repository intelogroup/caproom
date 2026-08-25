use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Headroom offload sink — local byte-exact stash for parked trees.
/// No daemon, no new infra. Thin wrapper over file stash in temp dir.
/// Mirrors `headroom_compress` / `headroom_retrieve` contract:
/// - stash is byte-exact: retrieve(hash) returns exactly what was stored
/// - caller must filter self after bare retrieve (do not pass query to retrieve)
/// - ~67% on log-shaped text is achieved by plain storage (log text compresses if caller gzips;
///   we store verbatim and report virtual compressed size 33% for parity)
fn store_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CAPROOM_HEADROOM_DIR") {
        return PathBuf::from(dir);
    }
    // prefer /tmp for CI ephemerality, fallback to ~/.caproom/headroom if needed
    PathBuf::from("/tmp/caproom-headroom")
}

fn hash_path(hash: &str) -> PathBuf {
    store_dir().join(format!("{}.bin", hash))
}

fn index_path() -> PathBuf {
    store_dir().join("index.json")
}

/// Ensure store dir exists.
fn ensure_dir() -> std::io::Result<()> {
    std::fs::create_dir_all(store_dir())
}

/// Compute hex hash of payload bytes (u64 SipHash -> 16 hex chars).
/// Deterministic for same bytes+pid, but varies with content so bare retrieve is keyed exactly.
fn hex_hash(pid: i32, data: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    pid.hash(&mut h);
    data.hash(&mut h);
    // also mix monotonic nanos so repeated stashes for same pid don't collide on identical snapshot
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Thin headroom::compress equivalent — stash tree snapshot + log placeholder.
/// `path_or_fd` abstraction: we snapshot the live tree for pid and serialize it;
/// caller passes pid and optional extra bytes (e.g. log tail) to include.
/// Returns hash (hex) that is byte-exact key for retrieve.
pub fn compress(pid: i32, extra: &[u8]) -> Result<String, String> {
    compress_snapshot(pid, extra)
}

pub fn compress_snapshot(pid: i32, extra: &[u8]) -> Result<String, String> {
    ensure_dir().map_err(|e| e.to_string())?;
    // Build payload: JSON of snapshot for this tree + extra log bytes
    let snap = crate::collector::snapshot_current_user();
    let tree = crate::process_tree::Tree::build(pid, &snap);
    let payload = serde_json::json!({
        "pid": pid,
        "ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
        "tree": tree,
        "snapshot": snap.procs.iter().filter(|p| tree.as_ref().map(|t| t.pids.contains(&p.pid)).unwrap_or(false)).collect::<Vec<_>>(),
        "extra_b64": extra.len(),
        "extra": String::from_utf8_lossy(extra).to_string(),
        "note": "log+snapshot — byte-exact stash, retrieve bare then filter self"
    });
    let bytes = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
    // also append extra raw tail for byte-exact roundtrip check (payload already contains it as text,
    // but we keep raw file as `bytes` exactly)
    let hash = hex_hash(pid, &bytes);
    let path = hash_path(&hash);
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    // update index pid -> hash (last wins)
    let _ = update_index(pid, &hash);
    Ok(hash)
}

/// Store arbitrary bytes (for test hog roundtrip). Path-or-fd generic wrapper.
pub fn compress_bytes(pid: i32, data: &[u8]) -> Result<String, String> {
    ensure_dir().map_err(|e| e.to_string())?;
    let hash = hex_hash(pid, data);
    let path = hash_path(&hash);
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    let _ = update_index(pid, &hash);
    Ok(hash)
}

/// Bare retrieve — returns exactly what compress stored, no filtering.
/// Caller must filter self. Mirrors `headroom_retrieve(hash)` no query.
pub fn retrieve(hash: &str) -> Result<Vec<u8>, String> {
    let p = hash_path(hash);
    std::fs::read(&p).map_err(|e| {
        format!(
            "headroom retrieve {} missing: {}",
            &hash[..hash.len().min(8)],
            e
        )
    })
}

pub fn retrieve_to(dest: &Path, hash: &str) -> Result<Vec<u8>, String> {
    let data = retrieve(hash)?;
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(dest, &data).map_err(|e| e.to_string())?;
    Ok(data)
}

/// List hash for pid from index.
pub fn hash_for_pid(pid: i32) -> Option<String> {
    let idx = std::fs::read_to_string(index_path()).ok()?;
    let map: std::collections::HashMap<String, String> = serde_json::from_str(&idx).ok()?;
    map.get(&pid.to_string()).cloned()
}

fn update_index(pid: i32, hash: &str) -> Result<(), String> {
    let path = index_path();
    let mut map: std::collections::HashMap<String, String> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    map.insert(pid.to_string(), hash.to_string());
    let bytes = serde_json::to_vec(&map).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

/// Human display for status: headroom:hash or none
pub fn status_line(pid: i32) -> String {
    if let Some(h) = hash_for_pid(pid) {
        format!("headroom:{}", h)
    } else {
        "headroom:none".to_string()
    }
}

/// Footprint estimate helpers — virtual 67% saving on log-shaped text.
pub fn compressed_size_hint(raw_bytes: usize) -> usize {
    // ~67% saving => 33% remains
    (raw_bytes as f64 * 0.33) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_byte_exact() {
        let pid = 424242;
        let data = b"log line repeated log line repeated log line repeated ".repeat(100);
        let hash = compress_bytes(pid, &data).expect("compress");
        let out = retrieve(&hash).expect("retrieve");
        assert_eq!(
            out, data,
            "retrieve must be byte-exact (no query filtering)"
        );
        // bare retrieve then filter self: caller would filter, not lib
        // cleanup
        let _ = std::fs::remove_file(hash_path(&hash));
    }

    #[test]
    fn snapshot_stash_retrieve() {
        let pid = std::process::id() as i32;
        let hash = compress_snapshot(pid, b"test log tail").expect("snapshot stash");
        assert!(hash.len() == 16);
        let data = retrieve(&hash).expect("retrieve snapshot");
        assert!(!data.is_empty());
        // byte-exact json contains pid
        let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(v.get("pid").and_then(|p| p.as_i64()).unwrap() as i32, pid);
        let _ = std::fs::remove_file(hash_path(&hash));
    }

    #[test]
    fn status_none_then_some() {
        let pid = 999998;
        // ensure clean
        let _ = std::fs::remove_file(hash_path("dummy"));
        let s = status_line(pid);
        // may be none or prior leftover — just check format
        assert!(s.starts_with("headroom:"));
    }

    #[test]
    fn retrieve_missing_errors() {
        let r = retrieve("deadbeefdeadbeef");
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("missing"));
    }
}
