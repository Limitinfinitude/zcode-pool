use serde_json::Value;
use std::time::Duration;

const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";
const SCOPE: &str = "https://graph.microsoft.com/.default";
const TIMEOUT_SECS: u64 = 25;
const MAX_TEXT: usize = 512 * 1024;
const MAX_LINKS: usize = 5;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .build()
}

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

pub struct Token {
    pub access_token: String,
    pub new_refresh_token: Option<String>,
}

const SCOPES_FALLBACK: [&str; 2] = [
    "https://graph.microsoft.com/Mail.Read offline_access",
    "https://graph.microsoft.com/User.Read offline_access",
];

fn try_exchange(client_id: &str, refresh_token: &str, scope: &str) -> Result<Token, String> {
    let resp = agent()
        .post(TOKEN_URL)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_form(&[
            ("client_id", client_id.trim()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.trim()),
            ("scope", scope),
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

pub fn exchange_token(client_id: &str, refresh_token: &str) -> Result<Token, String> {
    if client_id.trim().is_empty() {
        return Err(crate::i18n::tr("err.graph.no_client"));
    }
    if refresh_token.trim().is_empty() {
        return Err(crate::i18n::tr("err.graph.no_token"));
    }
    let mut first_err: Option<String> = None;
    for scope in std::iter::once(SCOPE).chain(SCOPES_FALLBACK.iter().copied()) {
        match try_exchange(client_id, refresh_token, scope) {
            Ok(t) => return Ok(t),
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    Err(first_err.unwrap_or_else(|| crate::i18n::tr("err.graph.token")))
}

pub fn is_credential_error(msg: &str) -> bool {
    msg.contains("invalid_grant")
        || msg.contains("AADSTS70000")
        || msg.contains("AADSTS70008")
        || msg.contains("AADSTS50173")
        || msg.contains("AADSTS7000215")
}

const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const DEVICE_SCOPE: &str = "https://graph.microsoft.com/.default offline_access";

pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

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
    pub pending: bool,
    pub refresh_token: Option<String>,
}

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

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&#38;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

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

fn dedup_keep_order(mut v: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    v.retain(|l| seen.insert(l.clone()));
    v
}

pub struct Found {
    pub links: Vec<String>,
    pub scanned: usize,
    pub newest_subject: Option<String>,
    pub newest_at: Option<String>,
}

fn received_ms(m: &Value) -> Option<i64> {
    m.get("receivedDateTime")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
}

pub fn fetch_links(
    client_id: &str,
    refresh_token: &str,
    top: usize,
    since_ms: Option<i64>,
) -> Result<(Found, Option<String>), String> {
    let take = top.clamp(1, 25);
    let tok = exchange_token(client_id, refresh_token)?;

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
    let mut newest_subject: Option<String> = None;
    let mut newest_at: Option<String> = None;
    if let Some(arr) = items {
        for m in arr {
            scanned += 1;
            if scanned == 1 {
                newest_subject = m
                    .get("subject")
                    .and_then(Value::as_str)
                    .map(|s| s.chars().take(80).collect::<String>());
                newest_at = m
                    .get("receivedDateTime")
                    .and_then(Value::as_str)
                    .map(String::from);
            }
            if let (Some(since), Some(at)) = (since_ms, received_ms(m)) {
                if at < since {
                    continue;
                }
            }
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
            newest_subject,
            newest_at,
        },
        tok.new_refresh_token,
    ))
}

/// 收件箱里的一封邮件（控制台「邮箱管理」页用）：给的是**正文纯文本**，
/// 前端直接当文本渲染，不做 HTML 注入。
pub struct MailMessage {
    pub id: String,
    pub subject: String,
    pub from: String,
    pub received_at: Option<String>,
    pub preview: String,
    pub body: String,
}

/// 把邮件 HTML 正文压成纯文本：去掉标签、丢掉 `<style>`/`<script>` 里的内容、
/// 解几个常见实体。够「看正文 / 找链接」用，不做完整 HTML 解析。
fn html_to_text(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < s.len() {
        if bytes[i] == b'<' {
            // 跳过整个 <style>/<script> 块（含内容），它们会污染正文
            let rest = &lower[i..];
            let skip_tag = if rest.starts_with("<style") {
                Some("</style>")
            } else if rest.starts_with("<script") {
                Some("</script>")
            } else {
                None
            };
            if let Some(end_tag) = skip_tag {
                if let Some(pos) = rest.find(end_tag) {
                    i += pos + end_tag.len();
                    out.push(' ');
                    continue;
                }
            }
            // 普通标签：跳到 '>' 之后，补个空格避免相邻文字粘连
            match s[i..].find('>') {
                Some(pos) => {
                    i += pos + 1;
                    out.push(' ');
                }
                None => break,
            }
            continue;
        }
        let ch = s[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 拉取收件箱最近若干封邮件（含正文）。和 `fetch_links` 一样会回落刷新的 refresh_token。
pub fn fetch_messages(
    client_id: &str,
    refresh_token: &str,
    top: usize,
) -> Result<(Vec<MailMessage>, Option<String>), String> {
    let take = top.clamp(1, 25);
    let tok = exchange_token(client_id, refresh_token)?;
    let url = format!(
        "{GRAPH}/me/messages?$top={take}&$select=id,subject,from,receivedDateTime,bodyPreview,body"
    );
    let body = agent()
        .get(&url)
        .set("Authorization", &format!("Bearer {}", tok.access_token))
        .call()
        .map_err(|e| http_err(&crate::i18n::tr("err.graph.list"), e))?
        .into_string()
        .unwrap_or_default();
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);

    let mut out = Vec::new();
    if let Some(arr) = v.get("value").and_then(Value::as_array) {
        for m in arr {
            let get = |p: &str| m.get(p).and_then(Value::as_str).unwrap_or("").to_string();
            let subject = {
                let s = get("subject");
                if s.is_empty() { "(无主题)".to_string() } else { s }
            };
            let from = m
                .pointer("/from/emailAddress/address")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let raw_body = m.pointer("/body/content").and_then(Value::as_str).unwrap_or("");
            let mut text = html_to_text(raw_body);
            if text.len() > 12000 {
                text.truncate(12000);
                text.push_str(" …");
            }
            let preview = {
                let p = get("bodyPreview");
                p.chars().take(200).collect::<String>()
            };
            out.push(MailMessage {
                id: get("id"),
                subject,
                from,
                received_at: m.get("receivedDateTime").and_then(Value::as_str).map(String::from),
                preview,
                body: text,
            });
        }
    }

    Ok((out, tok.new_refresh_token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_plain_link() {
        let hay = "点这里 https://chat.z.ai/verify?token=abc 完成";
        assert_eq!(scan_links(hay), vec!["https://chat.z.ai/verify?token=abc"]);
    }

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
