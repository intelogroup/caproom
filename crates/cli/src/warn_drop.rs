//! Drops a RAM-pressure warning as a file for the Claude Code UserPromptSubmit hook
//! (see `hooks/caproom-warn.py` in the repo root) to pick up and inject as
//! additionalContext on the user's own next prompt. Never touches the child's
//! terminal or stdin — purely a side-channel file drop.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

fn warn_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = std::path::Path::new(&home).join(".cache/caproom");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("pending_warning.json"))
}

pub fn write_warning(pid: i32, used_kb: u64, eff_kb: u64, free_pct: u8) {
    let Some(path) = warn_path() else { return };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let message = format!(
        "[caproom] RAM warning: this Claude Code session is using {}MB of its {}MB effective cap \
({}% free system-wide). It will be force-killed if usage keeps climbing. Wrap up now: save/commit \
important state, consider running /compact, and stop any heavy background tool calls.",
        used_kb / 1024,
        eff_kb / 1024,
        free_pct
    );
    let payload = serde_json::json!({
        "ts": ts,
        "pid": pid,
        "used_mb": used_kb / 1024,
        "limit_mb": eff_kb / 1024,
        "free_pct": free_pct,
        "message": message,
    });
    let tmp = path.with_extension("json.tmp");
    if let Ok(mut f) = std::fs::File::create(&tmp) {
        if f.write_all(payload.to_string().as_bytes()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}
