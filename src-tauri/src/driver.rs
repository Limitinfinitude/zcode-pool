use serde_json::{json, Map, Value};

pub const DRIVER_JS: &str = include_str!("driver/zpool-driver.js");

pub const SENTINEL_SCHEME: &str = "zpool-driver";
const SENTINEL_PREFIX: &str = "zpool-driver://msg";

pub const MODES: &[&str] = &["observe", "login", "register"];

pub const DEFAULT_MODE: &str = "login";

const KINDS: &[&str] = &["ready", "log", "phase", "note", "ask", "page"];

const STRING_FIELDS: &[&str] = &[
    "kind", "page", "phase", "code", "level", "ask", "a", "b", "url", "mode", "provider",
];

pub fn is_valid_mode(mode: &str) -> bool {
    MODES.contains(&mode)
}

pub fn normalize_mode(mode: Option<&str>) -> String {
    let m = mode.unwrap_or(DEFAULT_MODE).trim().to_ascii_lowercase();
    if is_valid_mode(&m) {
        m
    } else {
        DEFAULT_MODE.to_string()
    }
}

pub fn bootstrap_script(cfg: &Value) -> String {
    let json = serde_json::to_string(cfg).unwrap_or_else(|_| "{}".to_string());
    format!("window.__ZPOOL_CFG = {json};\n{DRIVER_JS}\n")
}

pub fn parse_message(url: &str) -> Option<Value> {
    let rest = url.strip_prefix(SENTINEL_PREFIX)?;
    let qs = rest.trim_start_matches('?');
    let mut map = Map::new();
    for kv in qs.split('&') {
        if kv.is_empty() {
            continue;
        }
        let (k, v) = match kv.split_once('=') {
            Some(p) => p,
            None => continue,
        };
        let key = urldecode(k);
        if !STRING_FIELDS.contains(&key.as_str()) {
            continue;
        }
        map.insert(key, Value::String(urldecode(v)));
    }
    let kind = map.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if !KINDS.contains(&kind) {
        return None;
    }
    for key in ["a", "b", "url"] {
        if let Some(Value::String(s)) = map.get(key) {
            if s.chars().count() > 1000 {
                map.insert(key.to_string(), Value::String(s.chars().take(1000).collect()));
            }
        }
    }
    Some(Value::Object(map))
}

pub fn closed_event() -> Value {
    json!({ "kind": "closed" })
}

pub fn summarize(msg: &Value) -> String {
    let get = |k: &str| msg.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let mut out = String::new();
    for k in ["kind", "level", "page", "phase", "code", "ask"] {
        let v = get(k);
        if !v.is_empty() {
            out.push_str(&format!("{k}={v} "));
        }
    }
    for k in ["a", "b", "url"] {
        let v = get(k);
        if !v.is_empty() {
            let t: String = v.chars().take(140).collect();
            out.push_str(&format!("{k}={t} "));
        }
    }
    out.trim_end().to_string()
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let hex = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    out.push(h << 4 | l);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_log_message() {
        let v = parse_message("zpool-driver://msg?kind=log&level=ok&code=clickedCreate&a=me%40z.ai")
            .unwrap();
        assert_eq!(v["kind"], "log");
        assert_eq!(v["level"], "ok");
        assert_eq!(v["code"], "clickedCreate");
        assert_eq!(v["a"], "me@z.ai");
    }

    #[test]
    fn parses_note_and_phase() {
        let v = parse_message("zpool-driver://msg?kind=note&code=settle").unwrap();
        assert_eq!(v["code"], "settle");
        let v = parse_message("zpool-driver://msg?kind=phase&phase=signup-form").unwrap();
        assert_eq!(v["phase"], "signup-form");
    }

    #[test]
    fn rejects_unknown_kind_and_foreign_scheme() {
        assert!(parse_message("zpool-driver://msg?kind=evil").is_none());
        assert!(parse_message("https://example.com/?kind=log").is_none());
        assert!(parse_message("zcode://oauth/callback?kind=log").is_none());
    }

    #[test]
    fn drops_non_whitelisted_fields() {
        let v = parse_message("zpool-driver://msg?kind=log&code=page&token=secret").unwrap();
        assert!(v.get("token").is_none());
        assert_eq!(v["code"], "page");
    }

    #[test]
    fn truncates_oversized_field() {
        let big = "x".repeat(5000);
        let v = parse_message(&format!("zpool-driver://msg?kind=log&code=page&a={big}")).unwrap();
        assert_eq!(v["a"].as_str().unwrap().chars().count(), 1000);
    }

    #[test]
    fn normalize_mode_defaults_to_login() {
        assert_eq!(normalize_mode(Some("register")), "register");
        assert_eq!(normalize_mode(Some("REGISTER")), "register");
        assert_eq!(normalize_mode(Some("nope")), "login");
        assert_eq!(normalize_mode(None), "login");
        assert!(!is_valid_mode("nope"));
    }

    #[test]
    fn summarize_is_one_compact_line() {
        let v = parse_message("zpool-driver://msg?kind=log&level=ok&code=clickedCreate&a=me%40z.ai")
            .unwrap();
        let s = summarize(&v);
        assert!(s.contains("kind=log"));
        assert!(s.contains("level=ok"));
        assert!(s.contains("code=clickedCreate"));
        assert!(s.contains("a=me@z.ai"));
        assert!(!s.contains('\n'));
    }

    #[test]
    fn summarize_truncates_long_values() {
        let big = "x".repeat(5000);
        let v = parse_message(&format!("zpool-driver://msg?kind=log&code=page&a={big}")).unwrap();
        let s = summarize(&v);
        assert!(s.chars().count() < 300);
    }
}
