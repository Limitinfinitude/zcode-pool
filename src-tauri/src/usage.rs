//! 使用记录：一行一个请求，落在 `<store_dir>/usage.jsonl`。
//!
//! 和 `flowlog`（原始流水，排错用）**不是一回事** —— 这里是结构化的、给「用量」页
//! 看的。写法照抄 flowlog：一个写锁 + 超限改名轮转，只是阈值放大到 32MB。
//!
//! 落盘时机见 `gateway::Meter`：流式响应要等 reader 读完才知道总时长 / 总 token，
//! 所以是**流结束那一刻**才写，不是 `handle_external` 返回时。

use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 超过这个大小就改名成 `usage.jsonl.old`（旧的直接删）
const MAX_BYTES: u64 = 32 * 1024 * 1024;
static WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn path_for(base: &Path) -> PathBuf {
    base.join("usage.jsonl")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageRecord {
    /// 请求发出时刻（`tried_at`）
    pub t: u64,
    /// 账号名
    pub acct: String,
    /// 客户端请求的模型（**映射前**）
    pub model: String,
    /// 上游实际模型 = `peek_model(&head)`，没拿到就 None
    #[serde(default)]
    pub up: Option<String>,
    #[serde(rename = "in", default)]
    pub tokens_in: u64,
    #[serde(rename = "out", default)]
    pub tokens_out: u64,
    /// 首字节延迟 ms（没读到任何字节则 None）
    #[serde(default)]
    pub ttfb: Option<u64>,
    /// 总时长 ms
    pub ms: u64,
    /// 这一发流出去的字节（不是累计值）
    pub bytes: u64,
    pub status: u16,
    /// 业务码：200 正文里的 `"code"`（1005 / 3012 之类），没有就空串
    #[serde(default)]
    pub code: String,
    /// 第几个候选
    pub tries: usize,
    /// 响应 ctype 里有没有 `event-stream`
    pub stream: bool,
    /// 这一发命中的模型映射（没有就 None）
    #[serde(default)]
    pub mapped: Option<String>,
}

/// 追加一行。超 32MB 先改名轮转再写 —— 和 `flowlog::append` 一个路子。
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

/// 读整个文件。文件不存在 → 空；坏行直接跳过，绝不让一行损坏拖垮整页。
pub fn read_all(path: &Path) -> Vec<UsageRecord> {
    let Ok(s) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    s.lines()
        .filter_map(|l| serde_json::from_str::<UsageRecord>(l.trim()).ok())
        .collect()
}

/// 「这一发算不算失败」。业务码 200/0 视作成功 —— 正常响应也可能带 `"code":200`，
/// 不能一看见 code 就判失败。
pub fn is_fail(r: &UsageRecord) -> bool {
    if r.status >= 400 {
        return true;
    }
    let c = r.code.trim();
    !c.is_empty() && c != "200" && c != "0"
}

/// 聚合：总览 + byModel / byAcct / byDay。32MB 上限，全读进内存聚合扛得住。
pub fn stats(path: &Path) -> Value {
    let rows = read_all(path);
    let mut n = 0u64;
    let mut t_in = 0u64;
    let mut t_out = 0u64;
    let mut fail = 0u64;
    let mut ttfb_sum = 0u64;
    let mut ttfb_cnt = 0u64;
    let mut ms_sum = 0u64;

    // k -> (n, in, out)
    let mut by_model: std::collections::BTreeMap<String, (u64, u64, u64)> = Default::default();
    let mut by_acct: std::collections::BTreeMap<String, (u64, u64, u64)> = Default::default();
    let mut by_day: std::collections::BTreeMap<String, (u64, u64, u64)> = Default::default();

    for r in &rows {
        n += 1;
        t_in += r.tokens_in;
        t_out += r.tokens_out;
        if is_fail(r) {
            fail += 1;
        }
        if let Some(tt) = r.ttfb {
            ttfb_sum += tt;
            ttfb_cnt += 1;
        }
        ms_sum += r.ms;
        let bump = |m: &mut std::collections::BTreeMap<String, (u64, u64, u64)>, k: String| {
            let e = m.entry(k).or_insert((0, 0, 0));
            e.0 += 1;
            e.1 += r.tokens_in;
            e.2 += r.tokens_out;
        };
        bump(&mut by_model, if r.model.is_empty() { "-".into() } else { r.model.clone() });
        bump(&mut by_acct, if r.acct.is_empty() { "-".into() } else { r.acct.clone() });
        bump(&mut by_day, day_key(r.t));
    }

    let rows_of = |m: std::collections::BTreeMap<String, (u64, u64, u64)>, desc: bool| -> Vec<Value> {
        let mut v: Vec<(String, (u64, u64, u64))> = m.into_iter().collect();
        if desc {
            v.sort_by(|a, b| (b.1).0.cmp(&(a.1).0));
        }
        v.into_iter()
            .take(50)
            .map(|(k, (n, i, o))| json!({ "k": k, "n": n, "in": i, "out": o }))
            .collect()
    };

    json!({
        "total": {
            "n": n,
            "in": t_in,
            "out": t_out,
            "fail": fail,
            "avgTtfb": if ttfb_cnt == 0 { Value::Null } else { json!(ttfb_sum / ttfb_cnt) },
            "avgMs": if n == 0 { Value::Null } else { json!(ms_sum / n) },
        },
        "byModel": rows_of(by_model, true),
        "byAcct": rows_of(by_acct, true),
        "byDay": rows_of(by_day, false),
    })
}

fn day_key(ms: u64) -> String {
    match chrono::Local.timestamp_millis_opt(ms as i64).single() {
        Some(dt) => dt.format("%Y-%m-%d").to_string(),
        None => "-".into(),
    }
}
