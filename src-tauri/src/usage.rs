use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_BYTES: u64 = 32 * 1024 * 1024;
static WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn path_for(base: &Path) -> PathBuf {
    base.join("usage.jsonl")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageRecord {

    pub t: u64,

    pub acct: String,

    pub model: String,

    #[serde(default)]
    pub up: Option<String>,
    #[serde(rename = "in", default)]
    pub tokens_in: u64,
    #[serde(rename = "out", default)]
    pub tokens_out: u64,

    #[serde(default)]
    pub cache: u64,

    #[serde(default)]
    pub ttfb: Option<u64>,

    pub ms: u64,

    pub bytes: u64,
    pub status: u16,

    #[serde(default)]
    pub code: String,

    pub tries: usize,

    pub stream: bool,

    #[serde(default)]
    pub mapped: Option<String>,

    #[serde(default)]
    pub key: String,
}

pub fn append(path: &Path, rec: &UsageRecord) {
    let Ok(line) = serde_json::to_string(rec) else {
        return;
    };
    let _guard = match WRITE_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if std::fs::metadata(path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let old = path.with_extension("jsonl.old");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(path, &old);
    }
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = f.write_all(line.as_bytes());
    let _ = f.write_all(b"\n");
}

pub fn read_all(path: &Path) -> Vec<UsageRecord> {
    let Ok(s) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    s.lines()
        .filter_map(|l| serde_json::from_str::<UsageRecord>(l.trim()).ok())
        .collect()
}

pub fn is_fail(r: &UsageRecord) -> bool {
    if r.status >= 400 {
        return true;
    }
    let c = r.code.trim();
    !c.is_empty() && c != "200" && c != "0"
}

pub fn stats(path: &Path) -> Value {
    let rows = read_all(path);
    let mut n = 0u64;
    let mut t_in = 0u64;
    let mut t_out = 0u64;
    let mut t_cache = 0u64;
    let mut fail = 0u64;
    let mut ttfb_sum = 0u64;
    let mut ttfb_cnt = 0u64;
    let mut ms_sum = 0u64;

    let today = day_key(chrono::Local::now().timestamp_millis() as u64);
    let (mut tn, mut t_in_t, mut t_out_t, mut t_cache_t, mut t_fail) = (0u64, 0u64, 0u64, 0u64, 0u64);

    let mut by_model: std::collections::BTreeMap<String, (u64, u64, u64, u64)> = Default::default();
    let mut by_acct: std::collections::BTreeMap<String, (u64, u64, u64, u64)> = Default::default();
    let mut by_day: std::collections::BTreeMap<String, (u64, u64, u64, u64)> = Default::default();
    let mut by_key: std::collections::BTreeMap<String, (u64, u64, u64, u64)> = Default::default();

    for r in &rows {
        n += 1;
        t_in += r.tokens_in;
        t_out += r.tokens_out;
        t_cache += r.cache;
        if is_fail(r) {
            fail += 1;
        }
        if let Some(tt) = r.ttfb {
            ttfb_sum += tt;
            ttfb_cnt += 1;
        }
        ms_sum += r.ms;
        let bump = |m: &mut std::collections::BTreeMap<String, (u64, u64, u64, u64)>, k: String| {
            let e = m.entry(k).or_insert((0, 0, 0, 0));
            e.0 += 1;
            e.1 += r.tokens_in;
            e.2 += r.tokens_out;
            e.3 += r.cache;
        };
        bump(&mut by_model, if r.model.is_empty() { "-".into() } else { r.model.clone() });
        bump(&mut by_acct, if r.acct.is_empty() { "-".into() } else { r.acct.clone() });
        let dk = day_key(r.t);
        if dk == today {
            tn += 1;
            t_in_t += r.tokens_in;
            t_out_t += r.tokens_out;
            t_cache_t += r.cache;
            if is_fail(r) {
                t_fail += 1;
            }
        }
        bump(&mut by_day, dk);
        bump(&mut by_key, if r.key.is_empty() { "-".into() } else { r.key.clone() });
    }

    let rows_of = |m: std::collections::BTreeMap<String, (u64, u64, u64, u64)>, desc: bool| -> Vec<Value> {
        let mut v: Vec<(String, (u64, u64, u64, u64))> = m.into_iter().collect();
        if desc {
            v.sort_by(|a, b| (b.1).0.cmp(&(a.1).0));
        }
        v.into_iter()
            .take(50)
            .map(|(k, (n, i, o, c))| json!({ "k": k, "n": n, "in": i, "out": o, "cache": c }))
            .collect()
    };

    json!({
        "total": {
            "n": n,
            "in": t_in,
            "out": t_out,
            "cache": t_cache,
            "fail": fail,
            "avgTtfb": if ttfb_cnt == 0 { Value::Null } else { json!(ttfb_sum / ttfb_cnt) },
            "avgMs": if n == 0 { Value::Null } else { json!(ms_sum / n) },
        },
        "today": { "k": today, "n": tn, "in": t_in_t, "out": t_out_t, "cache": t_cache_t, "fail": t_fail },
        "byModel": rows_of(by_model, true),
        "byAcct": rows_of(by_acct, true),
        "byKey": rows_of(by_key, true),
        "byDay": rows_of(by_day, false),
    })
}

fn day_key(ms: u64) -> String {
    match chrono::Local.timestamp_millis_opt(ms as i64).single() {
        Some(dt) => dt.format("%Y-%m-%d").to_string(),
        None => "-".into(),
    }
}
