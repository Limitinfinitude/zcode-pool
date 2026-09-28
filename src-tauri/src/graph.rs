use serde_json::Value;
use std::time::Duration;

const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";
const SCOPE: &str = "https://graph.microsoft.com/.default";
const TIMEOUT_SECS: u64 = 25;
const MAX_TEXT: usize = 512 * 1024;
const MAX_LINKS: usize = 5;
/// 单封邮件返回给控制台的 HTML 上限（原样保留版式，别压成纯文本）
const MAX_HTML: usize = 256 * 1024;
/// 单封邮件最多抠多少条链接
const MAX_EXTRACT: usize = 40;

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

/// 从邮件 HTML 里抠出来的一条链接：`url` 是目标，`text` 是 `<a>` 的显示文字
/// （裸链接没有文字，空串）。控制台把这几条单独列出来，点一下就能复制激活链接。
#[derive(Debug, Clone, serde::Serialize)]
pub struct MailLink {
    pub url: String,
    #[serde(default)]
    pub text: String,
}

/// 收件箱里的一封邮件（控制台「邮箱管理」页用）：
/// **同时**给原文 `html`、纯文本兜底 `text`、以及从 HTML 里抠出来的 `links`。
/// 不再把 HTML 压成纯文本 —— 那些邮件的用处就是取激活/验证链接，压完链接就没了。
pub struct MailMessage {
    pub id: String,
    pub subject: String,
    pub from: String,
    pub received_at: Option<String>,
    pub preview: String,
    pub html: String,
    pub text: String,
    pub links: Vec<MailLink>,
}

/// 把邮件 HTML 正文压成纯文本：去掉标签、丢掉 `<style>`/`<script>` 里的内容、
/// 解几个常见实体。只当 `text` 兜底用（比如邮件只有 HTML 时给个纯文本预览）。
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

/// 按字节上限截断，但**不切碎 UTF-8 字符**（切一半会 panic）
fn truncate_bytes(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push_str(" …");
}

/// 找 `http(s)://` 在字符串里的最早出现位置
fn next_http(hay: &str) -> Option<usize> {
    match (hay.find("https://"), hay.find("http://")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// URL 的结束位置：空白或这些「不可能出现在 URL 里」的字符
fn url_end(s: &str) -> usize {
    s.find(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | '<' | '>' | ')' | ']' | '}' | '\\' | '`' | '，' | '。' | '）' | '、' | '“' | '”'
            )
    })
    .unwrap_or(s.len())
}

fn trim_url_tail(mut link: String) -> String {
    while let Some(last) = link.chars().last() {
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '&' | '"' | '\'') {
            link.pop();
        } else {
            break;
        }
    }
    link
}

/// 扫出文本里所有 `http(s)` URL（不限定主机）。裸链接用，去重留给调用方。
fn scan_urls(hay: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut from = 0usize;
    while from < hay.len() {
        let Some(rel) = next_http(&hay[from..]) else {
            break;
        };
        let start = from + rel;
        let rest = &hay[start..];
        let end = url_end(rest);
        let link = trim_url_tail(html_unescape(&rest[..end]));
        if link.len() > 8 {
            out.push(link);
        }
        from = start + end.max(1);
    }
    out
}

/// 从一段开标签里取属性值（支持 `"`、`'`、裸值；大小写不敏感）
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0usize;
    while from + name.len() <= lower.len() {
        let rel = lower[from..].find(name)?;
        let at = from + rel;
        let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let after = &tag[at + name.len()..];
        let trimmed = after.trim_start();
        let Some(rest) = trimmed.strip_prefix('=') else {
            from = at + name.len();
            continue;
        };
        if !before_ok {
            from = at + name.len();
            continue;
        }
        let rest = rest.trim_start();
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'');
        let body = if quote.is_some() { &rest[1..] } else { rest };
        let end = quote
            .map(|q| body.find(q).unwrap_or(body.len()))
            .unwrap_or_else(|| body.find(|c: char| c == '>' || c.is_whitespace()).unwrap_or(body.len()));
        let val = &body[..end];
        if !val.is_empty() {
            return Some(val.to_string());
        }
        from = at + name.len();
    }
    None
}

/// 去掉标签留下可读文字（给 `<a>` 的显示文字用）
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    html_unescape(out.trim())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 抠 `<a href="http(s)...">显示文字</a>`，返回 `(url, 文字)`。
/// 只认 http(s)，`mailto:` 之类不要。
fn extract_anchors(html: &str) -> Vec<(String, String)> {
    let lower = html.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0usize;
    while i + 2 < html.len() {
        // 定位 "<a" 且后面是空白或 '>'（排除 <abbr> 这类）
        let is_a = lb[i] == b'<'
            && lb[i + 1] == b'a'
            && matches!(lb[i + 2], b' ' | b'\t' | b'\r' | b'\n' | b'>');
        if !is_a {
            i += 1;
            continue;
        }
        let Some(p) = html[i..].find('>') else { break };
        let tag_end = i + p;
        let open = &html[i..=tag_end];
        if let Some(href) = attr_value(open, "href") {
            let url = html_unescape(&href);
            if url.starts_with("http://") || url.starts_with("https://") {
                let inner_start = tag_end + 1;
                let close = lower[inner_start..]
                    .find("</a>")
                    .map(|q| inner_start + q)
                    .unwrap_or(inner_start);
                let text = strip_tags(&html[inner_start..close]);
                let text: String = text.chars().take(160).collect();
                out.push((url, text));
                i = close + 4;
                continue;
            }
        }
        i = tag_end + 1;
    }
    out
}

/// 汇总一封邮件的所有链接：先 `<a href>`（带显示文字），再补裸 http(s)。
/// 按 url 去重、保文档顺序。
pub fn extract_links(html: &str) -> Vec<MailLink> {
    let mut out: Vec<MailLink> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (url, text) in extract_anchors(html) {
        if seen.insert(url.clone()) {
            out.push(MailLink { url, text });
        }
        if out.len() >= MAX_EXTRACT {
            return out;
        }
    }
    for url in scan_urls(html) {
        if seen.insert(url.clone()) {
            out.push(MailLink { url, text: String::new() });
        }
        if out.len() >= MAX_EXTRACT {
            break;
        }
    }
    out
}

/// 拉取收件箱最近若干封邮件（含原文 HTML、纯文本、链接）。和 `fetch_links` 一样会回落刷新的 refresh_token。
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
            let mut html = raw_body.to_string();
            truncate_bytes(&mut html, MAX_HTML);
            let mut text = html_to_text(raw_body);
            truncate_bytes(&mut text, 12000);
            let links = extract_links(raw_body);
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
                html,
                text,
                links,
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

    #[test]
    fn extracts_anchor_with_text_and_bare_url() {
        let html = r#"<p>点<a href="https://chat.z.ai/auth/verify_email?token=abc&amp;email=a%40b.com">验证邮箱</a></p>
        <p>或访问 https://chat.z.ai/help 看看</p>"#;
        let links = extract_links(html);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].url, "https://chat.z.ai/auth/verify_email?token=abc&email=a%40b.com");
        assert_eq!(links[0].text, "验证邮箱");
        assert_eq!(links[1].url, "https://chat.z.ai/help");
        assert_eq!(links[1].text, "");
    }

    #[test]
    fn dedups_anchor_and_bare_same_url() {
        let html = r#"<a href="https://chat.z.ai/x">go</a> 以及 https://chat.z.ai/x"#;
        let links = extract_links(html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].text, "go");
    }

    #[test]
    fn skips_mailto_and_keeps_http_only() {
        let html = r#"<a href="mailto:support@z.ai">写信</a><a href="https://z.ai/a">a</a>"#;
        let links = extract_links(html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://z.ai/a");
    }
}
