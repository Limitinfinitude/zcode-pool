use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static WRITE_LOCK: Mutex<()> = Mutex::new(());

const MAX_BYTES: u64 = 256 * 1024;

pub fn init(base: &Path) {
    let dir = base.join("logs");
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = LOG_PATH.set(dir.join("oauth.log"));
    }
}

/// 分页读日志（控制台「日志」页用）。`offset` 从**最新一条往回数**（0 = 最后一行），
/// 返回 `[offset, offset + limit)` 这一段，内部按时间正序（旧 -> 新）。
/// 同时返回总行数，前端好算页数。日志最大 256KB，整体读进来再切够用了。
pub fn tail_page(offset: usize, limit: usize) -> (Vec<String>, usize) {
    let Some(path) = LOG_PATH.get() else {
        return (Vec::new(), 0);
    };
    let Ok(s) = std::fs::read_to_string(path) else {
        return (Vec::new(), 0);
    };
    let all: Vec<&str> = s.lines().collect();
    let total = all.len();
    let limit = limit.max(1);
    let end = total.saturating_sub(offset);
    let start = end.saturating_sub(limit);
    let lines = all[start..end].iter().map(|l| l.to_string()).collect();
    (lines, total)
}

pub fn log(flow: &str, event: &str, detail: &str) {
    let Some(path) = LOG_PATH.get() else { return };
    append(path, &format_line(flow, event, detail));
}

fn format_line(flow: &str, event: &str, detail: &str) -> String {
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let tag: String = flow.chars().take(8).collect();
    let clean: String = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        format!("{ts} [{tag}] {event}\n")
    } else {
        format!("{ts} [{tag}] {event} {clean}\n")
    }
}

fn append(path: &Path, line: &str) {
    let _guard = match WRITE_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if std::fs::metadata(path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let old = path.with_extension("log.old");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(path, &old);
    }
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = f.write_all(line.as_bytes());
}
