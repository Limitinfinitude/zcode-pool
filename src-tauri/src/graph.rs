//! 微软 Graph 直连读信 —— 替代外部邮箱聚合工具。
//!
//! 导入的账号文本自带 `client_id` + `refresh_token`，拿这两个就能直接和
//! 微软换 token、拉邮件，不需要任何外部邮箱程序在跑。
//!
//! 三步：refresh_token 换 access_token → 列最近几封邮件 → 从正文剖出
//! Z.ai 的验证链接（最新的那封优先）。
//!
//! 两个坑：
//!   1. `scope` 必须用 `https://graph.microsoft.com/.default`。用窄权限
//!      （如 `Mail.Read`）会被拒（AADSTS70000），实测如此。
//!   2. refresh_token 会轮换：换 token 时微软**可能**回一个新的。回了就要
//!      回写存库，否则下次失效。所以这里把新 token 一并带出去。
//!
//! 边界：只有链接会离开本模块。邮件正文 / 主题 / 收发件人一律不落日志。

use serde_json::Value;
use std::time::Duration;

const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";
/// 必须用 .default —— 窄 scope 会被 AADSTS70000 拒掉（实测）
const SCOPE: &str = "https://graph.microsoft.com/.default";
const TIMEOUT_SECS: u64 = 25;
/// 单次取信最多拼多少字符进扫描缓冲，防止一封信把内存拉爆
const MAX_TEXT: usize = 512 * 1024;
/// 一次最多带回去几条链接
const MAX_LINKS: usize = 5;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .build()
}

/// 把 ureq 的 4xx/5xx 挖出响应体里的 error_description，别只给一句「HTTP 400」。
fn http_err(what: &str, e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let detail = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("error_description")
                        .or_else(|| v.get("error"))
                        .or_else(|| v.get("message"))
                        .and_then(Value::as_str)
                        .map(String::from)
                })
                .unwrap_or_default();
            let detail: String = detail.chars().take(200).collect();
            if detail.is_empty() {
                format!("{what}: HTTP {code}")
            } else {
                format!("{what}: HTTP {code} {detail}")
            }
        }
        other => format!("{what}: {other}"),
    }
}

/// 换到的 access_token，外加轮换出来的新 refresh_token（没有则 None）。
pub struct Token {
    pub access_token: String,
    pub new_refresh_token: Option<String>,
}

/// 用 client_id + refresh_token 换 access_token。
pub fn exchange_token(client_id: &str, refresh_token: &str) -> Result<Token, String> {
    if client_id.trim().is_empty() {
        return Err(crate::i18n::tr("err.graph.no_client"));
    }
    if refresh_token.trim().is_empty() {
        return Err(crate::i18n::tr("err.graph.no_token"));
    }
    let resp = agent()
        .post(TOKEN_URL)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_form(&[
            ("client_id", client_id.trim()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.trim()),
            ("scope", SCOPE),
        ])
        .map_err(|e| http_err(&crate::i18n::tr("err.graph.token"), e))?;
    let v: Value = resp
        .into_json()
        .map_err(|e| crate::i18n::trf("err.graph.token_json", &[("e", &e.to_string())]))?;
    let at = v
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| crate::i18n::tr("err.graph.no_access_token"))?;
    let new_rt = v
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(String::from)
        .filter(|s| s != refresh_token.trim());
    Ok(Token {
        access_token: at.to_string(),
        new_refresh_token: new_rt,
    })
}

/// 这个报错是不是「凭据失效」（refresh_token 过期 / 被吊销）。
/// 调用方据此把邮箱标「需重新授权」，批量里自动跳过，不反复卡同一个号。
pub fn is_credential_error(msg: &str) -> bool {
    msg.contains("invalid_grant")
        || msg.contains("AADSTS70000")
        || msg.contains("AADSTS70008")
        || msg.contains("AADSTS50173")
        || msg.contains("AADSTS7000215")
}

const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
/// 要拿 refresh_token，必须带 `offline_access`
const DEVICE_SCOPE: &str = "https://graph.microsoft.com/.default offline_access";

pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

/// 起一个设备码授权流程（**拿新的 refresh_token 用**）。
///
/// 用邮箱自己的 client_id。设备码流不需要重定向 URI，桌面端最省事：
/// 用户去 microsoft.com/devicelogin 输码，这边轮询换 token。
pub fn device_code_begin(client_id: &str) -> Result<DeviceCode, String> {
    if client_id.trim().is_empty() {
        return Err(crate::i18n::tr("err.graph.no_client"));
    }
    let v: Value = agent()
        .post(DEVICE_CODE_URL)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_form(&[("client_id", client_id.trim()), ("scope", DEVICE_SCOPE)])
        .map_err(|e| http_err(&crate::i18n::tr("err.graph.device"), e))?
        .into_json()
        .map_err(|e| crate::i18n::trf("err.graph.token_json", &[("e", &e.to_string())]))?;
    Ok(DeviceCode {
        device_code: v.get("device_code").and_then(Value::as_str).unwrap_or("").to_string(),
        user_code: v.get("user_code").and_then(Value::as_str).unwrap_or("").to_string(),
        verification_uri: v
            .get("verification_uri")
            .and_then(Value::as_str)
            .unwrap_or("https://microsoft.com/devicelogin")
            .to_string(),
        interval: v.get("interval").and_then(Value::as_u64).unwrap_or(5),
        expires_in: v.get("expires_in").and_then(Value::as_u64).unwrap_or(900),
    })
}

pub struct DevicePoll {
    /// 还没授权（authorization_pending / slow_down）
    pub pending: bool,
    pub refresh_token: Option<String>,
}

/// 轮询设备码授权。pending 时 refresh_token 为 None；成功即带出新 token。
pub fn device_code_poll(client_id: &str, device_code: &str) -> Result<DevicePoll, String> {
    let resp = agent()
        .post(TOKEN_URL)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_form(&[
            ("client_id", client_id.trim()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
        ]);
    match resp {
        Ok(r) => {
            let v: Value = r.into_json().unwrap_or(Value::Null);
            let rt = v.get("refresh_token").and_then(Value::as_str).map(String::from);
            Ok(DevicePoll { pending: rt.is_none(), refresh_token: rt })
        }
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let parsed = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
            let err = parsed.get("error").and_then(Value::as_str).unwrap_or("");
            if err == "authorization_pending" || err == "slow_down" {
                Ok(DevicePoll { pending: true, refresh_token: None })
            } else {
                let desc = parsed
                    .get("error_description")
                    .and_then(Value::as_str)
                    .map(|s| s.chars().take(160).collect::<String>())
                    .unwrap_or_default();
                Err(format!("HTTP {code} {err} {desc}"))
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

/// HTML 正文里的实体还原。不还原的话 `&amp;` 会把 token 参数污染成
/// `&amp;token=`，点过去直接 404。
fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&#38;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// 从任意文本里剖出 chat.z.ai 的链接。
fn scan_links(hay: &str) -> Vec<String> {
    const NEEDLES: [&str; 2] = ["https://chat.z.ai/", "http://chat.z.ai/"];
    let mut out: Vec<String> = Vec::new();
    for needle in NEEDLES {
        let mut from = 0usize;
        while let Some(rel) = hay[from..].find(needle) {
            let start = from + rel;
            let rest = &hay[start..];
            let end = rest
                .find(|c: char| {
                    c.is_whitespace()
                        || matches!(
                            c,
                            '"' | '\''
                                | '<'
                                | '>'
                                | ')'
                                | ']'
                                | '}'
                                | '\\'
                                | '`'
                                | '，'
                                | '。'
                                | '）'
                                | '、'
                                | '“'
                                | '”'
                        )
                })
                .unwrap_or(rest.len());
            let mut link = html_unescape(&rest[..end]);
            while let Some(last) = link.chars().last() {
                if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '&' | '"' | '\'') {
                    link.pop();
                } else {
                    break;
                }
            }
            if link.len() > needle.len() {
                out.push(link);
            }
            from = start + end.max(1);
        }
    }
    out
}

/// 去重但保留首次出现的位置。列表按时间倒序，先扫到的就是最新那封。
fn dedup_keep_order(mut v: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    v.retain(|l| seen.insert(l.clone()));
    v
}

pub struct Found {
    /// 去重后的链接，保持发现顺序 —— `[0]` 是最新一封邮件里的。
    pub links: Vec<String>,
    /// 实际看过几封
    pub scanned: usize,
}

/// 取链接。`top` 是往 Graph 要几封（1~25）。
/// 返回 (链接, 轮换出的新 refresh_token)。
pub fn fetch_links(
    client_id: &str,
    refresh_token: &str,
    top: usize,
) -> Result<(Found, Option<String>), String> {
    let take = top.clamp(1, 25);
    let tok = exchange_token(client_id, refresh_token)?;

    // 列表接口直接 `$select=body` 一次拿全，省掉每封再拉一次详情。
    // `/me/messages` 覆盖整个邮箱（含垃圾箱），验证信被丢进垃圾箱也不会漏。
    let url = format!(
        "{GRAPH}/me/messages?$top={take}&$select=id,subject,receivedDateTime,body"
    );
    let body = agent()
        .get(&url)
        .set("Authorization", &format!("Bearer {}", tok.access_token))
        .call()
        .map_err(|e| http_err(&crate::i18n::tr("err.graph.list"), e))?
        .into_string()
        .unwrap_or_default();
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);

    let items = v.get("value").and_then(Value::as_array);
    let mut links: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    if let Some(arr) = items {
        for m in arr {
            scanned += 1;
            // 一封一封来：数组本身按 receivedDateTime 倒序，所以先扫到的就是最新的。
            let mut text = String::new();
            if let Some(c) = m.pointer("/body/content").and_then(Value::as_str) {
                text.push_str(c);
                text.push('\n');
            }
            if let Some(p) = m.pointer("/bodyPreview").and_then(Value::as_str) {
                text.push_str(p);
            }
            if text.len() > MAX_TEXT {
                text.truncate(MAX_TEXT);
            }
            for l in scan_links(&text) {
                if !links.contains(&l) {
                    links.push(l);
                }
            }
            if links.len() >= MAX_LINKS {
                break;
            }
        }
    }

    Ok((
        Found {
            links: dedup_keep_order(links),
            scanned,
        },
        tok.new_refresh_token,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_plain_link() {
        let hay = "点这里 https://chat.z.ai/verify?token=abc 完成";
        assert_eq!(scan_links(hay), vec!["https://chat.z.ai/verify?token=abc"]);
    }

    /// 真实抓包（Z.ai 验证邮件）：href 里 `&` 是 `&amp;`，且不能被 `&` 提前截断。
    #[test]
    fn extracts_real_zai_verify_link() {
        let hay = r#"<a href="https://chat.z.ai/auth/verify_email?token=verify-12ad29efae9e366dc7619c624b9b5311&amp;email=aubreyrenolds09%40outlook.com&amp;username=QIANWEN2&amp;language=zh">验证邮箱</a>"#;
        assert_eq!(
            scan_links(hay),
            vec!["https://chat.z.ai/auth/verify_email?token=verify-12ad29efae9e366dc7619c624b9b5311&email=aubreyrenolds09%40outlook.com&username=QIANWEN2&language=zh"]
        );
    }

    #[test]
    fn ignores_other_hosts_and_dedups_in_order() {
        let hay = "https://example.com/x https://chat.z.ai/b https://chat.z.ai/a https://chat.z.ai/b";
        assert_eq!(
            dedup_keep_order(scan_links(hay)),
            vec!["https://chat.z.ai/b", "https://chat.z.ai/a"]
        );
    }
}
