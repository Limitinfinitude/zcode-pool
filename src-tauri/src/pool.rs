//! 邮箱账号池：导入的 `email----password----client_id----refresh_token` 存在这里。
//!
//! 一个账号的密码 / refresh_token 是敏感物，**只留在后端**：给前端的列表用
//! `Summary`，只带邮箱和状态，不带密码和令牌。
//!
//! 「已验证 / 未验证」以**本地记录**为准：只有本工具真的跑通「注册 → 邮箱
//! 验证 → OAuth 入库」才置 `verified`。不查外部状态，快且不依赖网络。

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const STATUS_NEW: &str = "new";
pub const STATUS_VERIFIED: &str = "verified";
pub const STATUS_FAILED: &str = "failed";
/// 凭据失效（refresh_token 过期 / 被吊销）：需重新授权，批量里自动跳过。
pub const STATUS_INVALID: &str = "invalid";

/// 存库的完整账号（含敏感字段）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MailAccount {
    pub email: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub refresh_token: String,
    /// new / verified / failed
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub verified_at: Option<String>,
    /// 失败原因或备注，给人看的
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

fn default_status() -> String {
    STATUS_NEW.to_string()
}

/// 回给前端的摘要：不含密码 / 令牌。
#[derive(Serialize, Clone, Debug)]
pub struct Summary {
    pub email: String,
    pub status: String,
    pub verified_at: Option<String>,
    pub note: Option<String>,
    /// refresh_token 在不在（不在就没法直连取信）
    pub has_token: bool,
    pub created_at: String,
}

impl MailAccount {
    pub fn to_summary(&self) -> Summary {
        Summary {
            email: self.email.clone(),
            status: self.status.clone(),
            verified_at: self.verified_at.clone(),
            note: self.note.clone(),
            has_token: !self.refresh_token.trim().is_empty(),
            created_at: self.created_at.clone(),
        }
    }
}

fn pool_file(root: &Path) -> std::path::PathBuf {
    root.join("mail.json")
}

pub fn load(root: &Path) -> Vec<MailAccount> {
    let raw = match std::fs::read_to_string(pool_file(root)) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    // 存的是 `{ "accounts": [...] }`；也容忍直接是数组
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Array(a)) => {
            serde_json::from_value(serde_json::Value::Array(a)).unwrap_or_default()
        }
        Ok(v) => serde_json::from_value(v.get("accounts").cloned().unwrap_or(serde_json::Value::Array(vec![]))).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn save(root: &Path, accounts: &[MailAccount]) -> Result<(), String> {
    std::fs::create_dir_all(root)
        .map_err(|e| crate::i18n::trf("err.pool.write", &[("e", &e.to_string())]))?;
    let doc = serde_json::json!({
        "format": "zpool-mail-pool",
        "version": 1,
        "accounts": accounts,
    });
    let body = serde_json::to_string_pretty(&doc)
        .map_err(|e| crate::i18n::trf("err.pool.write", &[("e", &e.to_string())]))?;
    std::fs::write(pool_file(root), body)
        .map_err(|e| crate::i18n::trf("err.pool.write", &[("e", &e.to_string())]))
}

pub fn find<'a>(accounts: &'a [MailAccount], email: &str) -> Option<&'a MailAccount> {
    let e = email.trim().to_ascii_lowercase();
    accounts.iter().find(|a| a.email.to_ascii_lowercase() == e)
}

/// 改某个邮箱的状态与备注（找不到就静默返回）。
pub fn set_status(root: &Path, email: &str, status: &str, note: Option<String>) -> Result<(), String> {
    let mut accounts = load(root);
    if let Some(a) = find_mut(&mut accounts, email) {
        a.status = status.to_string();
        a.note = note;
        if status == STATUS_VERIFIED {
            a.verified_at = Some(crate::store::now_ts());
        }
        save(root, &accounts)?;
    }
    Ok(())
}

pub fn find_mut<'a>(accounts: &'a mut [MailAccount], email: &str) -> Option<&'a mut MailAccount> {
    let e = email.trim().to_ascii_lowercase();
    accounts.iter_mut().find(|a| a.email.to_ascii_lowercase() == e)
}

/// 解析 `email----password----client_id----refresh_token` 文本。
/// 只取前四段；空行、`#` 注释、缺邮箱的行跳过。容忍 UTF-8 BOM。
/// 返回 (邮箱, 密码, client_id, refresh_token) 四元组。
pub fn parse_lines(raw: &str) -> Vec<(String, String, String, String)> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split("----").collect();
        let email = f.first().copied().unwrap_or("").trim();
        if !email.contains('@') {
            continue;
        }
        let password = f.get(1).copied().unwrap_or("").trim();
        let client_id = f.get(2).copied().unwrap_or("").trim();
        let refresh_token = f.get(3).copied().unwrap_or("").trim();
        out.push((
            email.to_string(),
            password.to_string(),
            client_id.to_string(),
            refresh_token.to_string(),
        ));
    }
    out
}

/// 导入结果：新增几个、跳过几个（已存在）。
#[derive(Serialize, Clone, Debug, Default)]
pub struct ImportResult {
    pub added: usize,
    pub skipped: usize,
    pub total_parsed: usize,
}

/// 把文本导入到已有池子里。已存在（同邮箱）的**不覆盖**，只算跳过。
pub fn import(accounts: &mut Vec<MailAccount>, raw: &str, now: String) -> ImportResult {
    let parsed = parse_lines(raw);
    let mut r = ImportResult {
        total_parsed: parsed.len(),
        ..Default::default()
    };
    for (email, password, client_id, refresh_token) in parsed {
        if find(accounts, &email).is_some() {
            r.skipped += 1;
            continue;
        }
        accounts.push(MailAccount {
            email,
            password,
            client_id,
            refresh_token,
            status: STATUS_NEW.to_string(),
            verified_at: None,
            note: None,
            created_at: now.clone(),
        });
        r.added += 1;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_four_fields() {
        let raw = "HoflerOdette15@outlook.com----vzzjlhi7265----9e5f94bc-e8a4----M.C534_token";
        let v = parse_lines(raw);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "HoflerOdette15@outlook.com");
        assert_eq!(v[0].1, "vzzjlhi7265");
        assert_eq!(v[0].2, "9e5f94bc-e8a4");
        assert_eq!(v[0].3, "M.C534_token");
    }

    #[test]
    fn skips_bom_blank_comment_and_malformed() {
        let raw = "\u{feff}a@b.com----p1----c----r\n\n# c\nnot-an-email----pw\nfoo@bar.com\n";
        let v = parse_lines(raw);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].0, "a@b.com");
        assert_eq!(v[1].0, "foo@bar.com");
        assert_eq!(v[1].1, "");
    }

    #[test]
    fn import_dedups_by_email_keeps_existing() {
        let mut acc = vec![MailAccount {
            email: "a@b.com".into(),
            password: "old".into(),
            client_id: String::new(),
            refresh_token: String::new(),
            status: STATUS_VERIFIED.into(),
            verified_at: None,
            note: None,
            created_at: String::new(),
        }];
        let r = import(&mut acc, "a@b.com----new----c----r\nc@d.com----p----c----r\n", "now".into());
        assert_eq!(r.added, 1);
        assert_eq!(r.skipped, 1);
        assert_eq!(acc.len(), 2);
        assert_eq!(acc[0].password, "old", "已存在的不能被覆盖");
        assert_eq!(acc[1].status, STATUS_NEW);
    }
}
