use crate::store::{self, Paths};
use sha2::{Digest, Sha256};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const UPSTREAM: &str = "https://zcode.z.ai/api/v1/zcode-plan/anthropic";
pub const DEFAULT_PORT: u16 = 8899;
const SWITCH_STATUS: [u16; 4] = [401, 402, 403, 429];
const MAX_TRIES: usize = 4;
const QUOTA_TTL_MS: u64 = 5 * 60 * 1000;
/// 上游说「这个号这个模型用尽了」之后，多久内不再挑它（和额度缓存 TTL 对齐）
const NO_QUOTA_TTL_MS: u64 = 5 * 60 * 1000;
/// 上游回 429 / 连不上 之后冷却多久（第三方实践：300 秒）
const RATE_TTL_MS: u64 = 5 * 60 * 1000;

pub const POLICY_EXPIRE: &str = "expire_first";
pub const POLICY_MOST_LEFT: &str = "most_left";
pub const POLICY_PINNED: &str = "pinned";

pub const MODEL_FOLLOW: &str = "follow";
pub const MODEL_AUTO: &str = "auto";

/* ---------------------------------------------------------------- 对外接口（反代） */

/// 对外可用的模型 id。前两个是常用的。
pub const EXTERNAL_MODELS: [&str; 2] = ["GLM-5.3-Flash", "GLM-5.3"];

// ZCode 那份真 system prompt：上游风控只认它，而且必须排在最前面 ——
// 换成别的、或者把调用方自己的 system 前置，都会 3012。
// 这份提示词不是我们的资产，运行时从本机 ZCode 安装里抽（见 `crate::prompt`），
// 抽不到就整体失败，绝不发一个没有正确 system 的请求出去。
// 完整对照实验见 `E:\zcode-switch\记录\06-外部应用接入号池.md`。
/// 取码页：由中继自己吐出来，工具开个小窗指过来（阿里云无痕验证需要真实浏览器环境）
const MINT_PAGE: &str = include_str!("../assets/mint.html");
/// 对话页：`/`。直接用反代打上游，顺手当「反代通不通」的验尸工具
const CHAT_PAGE: &str = include_str!("../assets/chat.html");
/// 控制台：`/proxy`。看状态、接入口、用量
const PROXY_PAGE: &str = include_str!("../assets/proxy.html");

/// 客户端请求头模板（从真客户端抓包抄的，逐字不改）。`sec-fetch-mode` 也照抄。
const CLIENT_HEADERS: [(&str, &str); 17] = [
    ("anthropic-version", "2023-06-01"),
    ("content-type", "application/json"),
    ("http-referer", "https://zcode.z.ai"),
    ("user-agent", "ZCode/3.14.3 ai-sdk/provider-utils/4.0.27 runtime/node.js/24"),
    ("x-client-language", "zh-CN"),
    ("x-client-timezone", "Asia/Shanghai"),
    ("x-os-category", "windows"),
    ("x-os-version", "10.0.26200"),
    ("x-platform", "win32-x64"),
    ("x-release-channel", "production"),
    ("x-title", "Z Code@electron"),
    ("x-zcode-agent", "glm"),
    ("x-zcode-app-version", "3.14.3"),
    ("x-zcode-session-type", "main"),
    ("accept", "*/*"),
    ("accept-language", "*"),
    ("sec-fetch-mode", "cors"),
];

/// 阿里云验证码区域（服务端 `getCaptchaConfig()` 下发的就是 cn）
const CAPTCHA_REGION: &str = "cn";

/* 取码节奏：**一个模型请求取一个码**，对齐客户端的自然节奏。多取是纯浪费
   （阿里云按次计，风控阈值未知）。预存只在「刚用过号」的活跃窗口里生效。 */
/// 速度采样的保留窗口。超了就当作「没在生成」，速度归零 —— 免得面板显示一个
/// 早就不动的旧数字，看着像还在跑。
const METER_WINDOW_MS: u64 = 15_000;
/// 健康探测的间隔。**带抖动**（见 probe_tick）：固定节奏的自动请求本身就是机器人特征，
/// 这条是风控上换来的教训，不是洁癖。
const PROBE_EVERY_MS: u64 = 90_000;
/// 每个号留多少条近期探测历史
const PROBE_HISTORY: usize = 24;
const PARAM_PRE: usize = 1;
const PARAM_MAX_POOL: usize = 4;
const PARAM_MAX_AGE_MS: u64 = 90_000;
const PARAM_WAIT_MS: u64 = 25_000;
const ACTIVE_WINDOW_MS: u64 = 180_000;
const INFLIGHT_TTL_MS: u64 = 30_000;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

struct Route {
    account: String,
    model: String,
    tries: usize,
}

#[derive(Default)]
struct Inner {
    running: bool,
    port: u16,
    served: u64,
    switched: u64,
    client_account: Option<String>,
    last_route: Option<Route>,
    stop: Option<Arc<AtomicBool>>,
    /// 服务器的句柄。**必须留着** —— 只置停止标志的话，服务线程阻塞在
    /// `recv_timeout` 上不一定立刻醒，旧监听会赖着不走（实测切监听地址后
    /// 0.0.0.0 和 127.0.0.1 同时在听）。stop() 里靠它 unblock()。
    server: Option<Arc<tiny_http::Server>>,
    /// 服务线程是否已经真的退出。stop() 要等它 —— 不然切端口/监听地址时
    /// 旧 socket 还没释放，新的一起就会出「地址被占用」或者两个都在听。
    serving: Option<Arc<AtomicBool>>,
    quota: HashMap<String, (u64, crate::quota::QuotaOverview)>,
    sticky: Option<String>,
    refreshing: bool,
    last_refresh: Option<String>,
    last_billing: Option<String>,
    policy: String,
    pinned: Option<String>,
    model_mode: String,
    quota_total: usize,
    last_failed: Vec<String>,
    /// 对外提供接口（反代）。关着时外部请求一律拒 —— 免得本机谁都能来烧额度
    external: bool,
    /// 取码池：`(param, region, 拿到时刻 ms)`。param 是一次性的、服务端不给票据，只能现场取
    params: Vec<(String, String, u64)>,
    /// 正在等码的请求数
    waiters: usize,
    /// 已派给取码页、还没交回来的活（多个取码窗同时长轮询时防重复派单）
    inflight: Vec<u64>,
    /// 上次真的用掉一个码的时刻（活跃窗口的起点）
    last_use: u64,
    mints: u64,
    mint_times: Vec<u64>,
    /// 流给调用方的字节数（累计）
    bytes_out: u64,
    /// 从 SSE 的 usage 里读到的最新 output_tokens
    tokens_out: u64,
    /// 速度采样 `(时刻 ms, 累计字节, 累计 token)`
    meter: Vec<(u64, u64, u64)>,
    /// 账号 id -> 健康快照
    health: HashMap<String, Health>,
    /// 最近一发模型请求的首字节延迟 / 总时长（数据面延迟，和探测那种控制面延迟不是一回事）
    last_ttfb_ms: Option<u64>,
    last_total_ms: Option<u64>,
    /// 监听地址：`127.0.0.1` 只本机 / `0.0.0.0` 局域网可访问
    bind: String,
    /// 对外接口的密钥。**绑非回环地址时必须非空**
    keys: Vec<String>,
    /// 模型映射：客户端请求的模型名 -> 上游真正的模型名
    model_map: std::collections::BTreeMap<String, String>,
    /// (账号 id, 模型 key) -> 上游判「用尽」的时刻
    no_quota: HashMap<(String, String), u64>,
    /// 账号 id -> 冷却到期时刻（只用于 429 这类「这个号现在不行」的情况）
    blocked: HashMap<String, u64>,
}

#[derive(Clone, Default)]
pub struct Gateway {
    inner: Arc<Mutex<Inner>>,
    /// exe 的 AppHandle。反代跑在独立线程里，默认碰不到窗口/托盘；
    /// `lib.rs` 的 setup 钩子把它塞进来，网页就能触发「弹验证码窗」「刷托盘」这类 exe 侧动作。
    app: Arc<std::sync::OnceLock<tauri::AppHandle>>,
}

struct Candidate {
    id: String,
    name: String,
    token: String,
    mid: String,
    expire: Option<String>,
    left: f64,
}

impl Gateway {
    pub fn new() -> Self {
        let g = Self::default();
        {
            let mut i = g.inner.lock().unwrap();
            i.bind = "127.0.0.1".to_string();
            i.policy = POLICY_EXPIRE.to_string();
            i.model_mode = MODEL_FOLLOW.to_string();
        }
        g
    }

    /// setup 钩子把 AppHandle 交给反代线程（只塞一次）
    pub fn set_app(&self, app: tauri::AppHandle) {
        let _ = self.app.set(app);
    }

    fn app(&self) -> Option<tauri::AppHandle> {
        self.app.get().cloned()
    }

    /// 网页改了账号（切换/删除/重命名/新建）后，把托盘 tooltip/菜单和 exe 面板同步一下。
    /// 没拿到 AppHandle 就静默跳过 —— 顶多是 exe 面板慢半拍，不影响网页操作本身。
    fn sync_exe(&self) {
        if let Some(app) = self.app() {
            crate::rebuild_tray(&app);
            crate::emit_state_changed(&app);
        }
    }

    /// 控制台 / 对话页用的轻量状态。
    /// 比 `status()` 小得多（不带额度明细），页面两秒轮询一次也不心疼。
    pub fn console_status(&self, paths: &Paths) -> Value {
        let accs = store::list_accounts(paths).unwrap_or_default();
        let accounts: Vec<String> = accs.iter().map(|a| a.name.clone()).collect();
        let mut g = self.inner.lock().unwrap();
        let now = now_ms();
        g.meter.retain(|s| now.saturating_sub(s.0) < METER_WINDOW_MS);
        let (dt, dbytes, dtok) = meter_delta(&g);
        // 抠不到 usage 就按 4 字节/token 粗估（中英混排的经验值），并把单位标出来
        let (tps, tps_est) = if dt <= 0.0 {
            (0.0, false)
        } else if dtok > 0 {
            (dtok as f64 / dt, false)
        } else {
            (dbytes as f64 / 4.0 / dt, true)
        };
        let bps = if dt <= 0.0 { 0.0 } else { dbytes as f64 / dt };
        // 哪些号现在被本地冻着（限流/用尽）—— 控制台要能一眼看到并手动解冻
        let frozen: serde_json::Map<String, Value> = {
            let mut m = serde_json::Map::new();
            for (id, t) in g.blocked.iter() {
                if *t > now {
                    m.insert(id.clone(), json!("限流"));
                }
            }
            for ((id, _), t) in g.no_quota.iter() {
                if now.saturating_sub(*t) < NO_QUOTA_TTL_MS {
                    m.insert(id.clone(), json!("用尽"));
                }
            }
            m
        };
        let generating = g
            .meter
            .last()
            .map(|s| now.saturating_sub(s.0) < 2500)
            .unwrap_or(false);
        let routing = g
            .last_route
            .as_ref()
            .map(|r| r.account.clone())
            .unwrap_or_else(|| "-".into());
        let routed = g
            .last_route
            .as_ref()
            .map(|r| json!({ "account": r.account, "model": r.model, "tries": r.tries }));
        json!({
            "running": g.running,
            "port": g.port,
            "external": g.external,
            "served": g.served,
            "switched": g.switched,
            "models": EXTERNAL_MODELS,
            "mint": mint_stats(&g),
            "routed": routed,
            "routing": routing,
            "accounts": accounts,
            // 渠道页要的是 id（health/frozen 都按 id 索引）+ 名字
            "channels": accs.iter().map(|a| json!({
                "id": a.id,
                "name": a.name,
                "health": g.health.get(&a.id).map(|h| h.json()),
                "frozen": frozen.get(&a.id),
            })).collect::<Vec<_>>(),
            "bytes": g.bytes_out,
            "tokens": g.tokens_out,
            "tps": (tps * 10.0).round() / 10.0,
            "tpsEstimated": tps_est,
            "bps": (bps * 10.0).round() / 10.0,
            "generating": generating,
            "ttfbMs": g.last_ttfb_ms,
            "totalMs": g.last_total_ms,
            "bind": g.bind,
            "keys": g.keys,
            "modelMap": g.model_map,
            "lanHost": lan_host(),
            "policy": g.policy,
            "pinned": g.pinned,
            "modelMode": g.model_mode,
            "health": g.health.iter().map(|(k, v)| (k.clone(), v.json())).collect::<serde_json::Map<String, Value>>(),
            "frozen": frozen,
            // 管理页要看额度缓存状态（原来只有更重的 status() 带这些）
            "quotaAccounts": g.quota.len(),
            "quotaTotal": g.quota_total,
            "quotaFailed": g.last_failed.clone(),
            "lastRefresh": g.last_refresh.clone(),
        })
    }

    pub fn status(&self, paths: &Paths) -> Value {
        let next_up = if self.inner.lock().unwrap().last_route.is_none() {
            self.next_up(paths)
        } else {
            None
        };
        let live_id = if self.inner.lock().unwrap().client_account.is_none() {
            live_account_id(paths)
        } else {
            None
        };
        let g = self.inner.lock().unwrap();
        let client_id = g.client_account.clone().or(live_id);
        let route_id = g
            .last_route
            .as_ref()
            .map(|r| r.account.clone())
            .or_else(|| next_up.clone());
        json!({
            "running": g.running,
            "port": g.port,
            "defaultPort": DEFAULT_PORT,
            "served": g.served,
            "switched": g.switched,
            "upstream": UPSTREAM,
            "clientAccount": client_id.clone(),
            "routedAccount": g.last_route.as_ref().map(|r| r.account.clone()),
            "routedModel": g.last_route.as_ref().map(|r| r.model.clone()),
            "routedTries": g.last_route.as_ref().map(|r| r.tries),
            "nextAccount": next_up,
            "quotaAccounts": g.quota.len(),
            "quotaTotal": g.quota_total,
            "quotaFailed": g.last_failed.clone(),
            "lastRefresh": g.last_refresh,
            "lastBilling": g.last_billing,
            "routedQuota": quota_rows_for(&g, route_id.as_deref()),
            "clientQuota": quota_rows_for(&g, client_id.as_deref()),
            "blocked": g.blocked.values().filter(|t| **t > now_ms()).count(),
            "policy": g.policy,
            "pinned": g.pinned,
            "sticky": g.sticky,
            "modelMode": g.model_mode,
            "models": models_seen(&g.quota),
            "external": g.external,
            "externalModels": EXTERNAL_MODELS,
            "mint": mint_stats(&g),
        })
    }

    fn next_up(&self, paths: &Paths) -> Option<String> {
        let ms = {
            let g = self.inner.lock().unwrap();
            models_seen(&g.quota)
        };
        for m in ms {
            if let Some(c) = self.sticky_order(self.candidates(paths, &m)).into_iter().next() {
                return Some(c.id);
            }
        }
        None
    }

    pub fn warm_quota(
        &self,
        paths: &Paths,
        max_passes: usize,
        progress: impl Fn(Value),
    ) -> Value {
        let ids: Vec<String> = store::list_accounts(paths)
            .map(|v| v.into_iter().map(|a| a.id).collect())
            .unwrap_or_default();
        let total = ids.len();
        let mut pass = 0usize;
        let mut have = 0usize;
        for p in 1..=max_passes.max(1) {
            pass = p;
            let _ = self.refresh_quota(paths, false);
            have = {
                let g = self.inner.lock().unwrap();
                ids.iter().filter(|i| g.quota.contains_key(*i)).count()
            };
            progress(json!({ "pass": p, "have": have, "total": total }));
            if have >= total {
                break;
            }
            if p < max_passes {
                std::thread::sleep(Duration::from_millis(900));
            }
        }
        let failed = self.inner.lock().unwrap().last_failed.clone();
        json!({ "have": have, "total": total, "passes": pass,
                "complete": have >= total, "failed": failed })
    }

    pub fn set_policy(&self, policy: String, pinned: Option<String>) -> Result<(), String> {
        if !matches!(policy.as_str(), POLICY_EXPIRE | POLICY_MOST_LEFT | POLICY_PINNED) {
            return Err(crate::i18n::tr("err.gw.bad_policy"));
        }
        let mut g = self.inner.lock().unwrap();
        g.policy = policy;
        g.pinned = pinned.filter(|p| !p.trim().is_empty());
        g.sticky = None;
        Ok(())
    }

    pub fn set_model_mode(&self, mode: String) -> Result<(), String> {
        if !matches!(mode.as_str(), MODEL_FOLLOW | MODEL_AUTO) {
            return Err(crate::i18n::tr("err.gw.bad_policy"));
        }
        self.inner.lock().unwrap().model_mode = mode;
        Ok(())
    }

    /// 把账号放进冷却：3012 用 30 分钟，429/连不上 用 5 分钟，取更长的那个
    fn mark_blocked(&self, id: &str, ms: u64) {
        let mut g = self.inner.lock().unwrap();
        let now = now_ms();
        g.blocked.retain(|_, t| *t > now);
        let cur = g.blocked.get(id).copied().unwrap_or(0);
        g.blocked.insert(id.to_string(), cur.max(now + ms));
    }

    /// 探一个号：打一发**额度接口**（免费、不用验证码），量往返延迟。
    ///
    /// 为什么不用模型请求当探测：那是真花钱的（吃号的额度）。sub2api 的监控是给
    /// API-key 渠道用的，那边一次探测成本可忽略；这边是订阅号池，探测必须免费。
    /// 额度接口能同时回答「号还活着吗」「凭证还有效吗」「余额多少」。
    fn probe_one(&self, paths: &Paths, id: &str) -> Result<u64, String> {
        let secret = crate::zcrypto::default_secret(&paths.home);
        let acc = store::load_account(paths, id)?;
        let tokens = crate::quota::candidate_tokens(&acc.credentials, acc.config.as_ref(), &secret);
        if tokens.is_empty() {
            return Err("没有可用凭证".into());
        }
        let mid = store::account_mid(paths, id).ok();
        let t0 = now_ms();
        let r = crate::quota::query_quota_mid(&tokens, mid.as_deref());
        let ms = now_ms().saturating_sub(t0);
        let out = r.map(|_| ms).map_err(|e| e.chars().take(120).collect::<String>());
        let now = now_ms();
        let mut g = self.inner.lock().unwrap();
        let h = g.health.entry(id.to_string()).or_default();
        h.last_at = Some(now);
        h.last_ms = out.as_ref().ok().copied();
        match &out {
            Ok(_) => {
                h.ok += 1;
                h.streak = 0;
                h.last_err.clear();
            }
            Err(e) => {
                h.fail += 1;
                h.streak = h.streak.saturating_add(1);
                h.last_err = e.clone();
            }
        }
        h.history.push((now, out.as_ref().ok().copied()));
        while h.history.len() > PROBE_HISTORY {
            h.history.remove(0);
        }
        out
    }

    /// 挑最久没探过的号，探一个。间隔带 ±40% 抖动 —— 让节奏不成形状。
    fn probe_tick(&self, paths: &Paths) {
        let accounts = store::list_accounts(paths).unwrap_or_default();
        if accounts.is_empty() {
            return;
        }
        let pick = {
            let g = self.inner.lock().unwrap();
            accounts
                .iter()
                .map(|a| (g.health.get(&a.id).and_then(|h| h.last_at).unwrap_or(0), a.id.clone()))
                .min()
                .map(|(_, id)| id)
        };
        let Some(id) = pick else { return };
        // 抖动：把「每次都是 90 秒整」这个形状打掉
        let jitter = (now_ms() % (PROBE_EVERY_MS * 8 / 10)) / 2;
        std::thread::sleep(Duration::from_millis(jitter));
        match self.probe_one(paths, &id) {
            Ok(ms) => crate::flowlog::log("relay", "probe-ok", &format!("{id} {ms}ms")),
            Err(e) => crate::flowlog::log("relay", "probe-fail", &format!("{id} {e}")),
        }
    }

    /// 反代总开关。**HTTP 路由和 exe 命令共用这一个**，别写成两套逻辑。
    ///
    /// ⚠ 从网页关掉 = 服务停 = 控制台自己立刻失联。前端要能显示「已停止」而不是一直转圈。
    pub fn set_external_and_run(&self, paths: &Paths, on: bool) -> Result<Value, String> {
        if on {
            let running = self.inner.lock().unwrap().running;
            if !running {
                let port = self.port_or_default();
                self.start(Paths { home: paths.home.clone() }, port)?;
            }
        }
        self.set_external(on);
        // 记住这个开关 —— 重启不用再点一次。持久化跟状态变更放一起，
        // 这样 exe 命令和网页接口都自动有，不会一边记一边不记。
        if let Err(e) = store::set_relay_external(paths, on) {
            crate::flowlog::log("relay", "external-persist-fail", &e);
        }
        if !on {
            self.stop();
        }
        Ok(self.console_status(paths))
    }

    fn port_or_default(&self) -> u16 {
        match self.inner.lock().unwrap().port {
            0 => DEFAULT_PORT,
            p => p,
        }
    }

    /// 启动时把持久化的服务级设置读回来。
    ///
    /// ⚠ 只存不读等于没存 —— 之前监听地址/密钥/模型映射都是存了但重启就丢，
    /// 所以这里统一收口，别让 lib.rs 去摸 Inner 的内部字段。
    pub fn restore_persisted(&self, paths: &Paths) {
        let st = store::load_settings(paths);
        let mut g = self.inner.lock().unwrap();
        if let Some(b) = st.relay_bind {
            g.bind = b;
        }
        if let Some(k) = st.relay_keys {
            g.keys = k;
        }
        if let Some(m) = st.model_map {
            g.model_map = m;
        }
    }

    /// 加/删一条模型映射。`to` 传空 = 删掉这条。
    ///
    /// 用途：官方**换模型名 / 下线某模型**时，在网关里改名，而不是去改每个客户端。
    pub fn set_model_map(&self, paths: &Paths, from: &str, to: &str) -> Result<Value, String> {
        let from = from.trim();
        if from.is_empty() {
            return Err("请求的模型名不能空".into());
        }
        let mut g = self.inner.lock().unwrap();
        if to.trim().is_empty() {
            g.model_map.remove(from);
        } else {
            let to = to.trim();
            if to == from {
                return Err("映射到自己是多余的".into());
            }
            g.model_map.insert(from.to_string(), to.to_string());
        }
        let m = g.model_map.clone();
        drop(g);
        store::set_model_map(paths, &m)?;
        Ok(self.console_status(paths))
    }

    /// 改监听地址。`0.0.0.0` = 局域网可访问。
    ///
    /// ⚠ **硬护栏**：绑非回环地址时至少要有一个密钥。否则同网段任何人
    /// 都能拿你的号池打模型，把额度烧光 —— 这个组合不该被允许存在。
    pub fn set_bind(&self, paths: &Paths, bind: &str) -> Result<Value, String> {
        let bind = bind.trim().to_string();
        let loopback = matches!(bind.as_str(), "127.0.0.1" | "localhost" | "::1");
        if !loopback {
            let n = self.inner.lock().unwrap().keys.len();
            if n == 0 {
                return Err("绑到非本机地址之前，先创建一个 API Key —— 否则同网段的人都能用你的号池".into());
            }
        }
        if let Err(e) = store::set_relay_bind(paths, &bind) {
            crate::flowlog::log("relay", "bind-persist-fail", &e);
        }
        {
            let mut g = self.inner.lock().unwrap();
            g.bind = bind.clone();
        }
        // 不用重启服务：socket 一直绑在通配地址上，这里只翻策略标志 —— 立刻生效、零风险
        crate::flowlog::log(
            "relay",
            "bind-set",
            if loopback { "监听策略 -> 仅本机" } else { "监听策略 -> 局域网可访问（要带 key）" },
        );
        Ok(self.console_status(paths))
    }

    /// 创建 / 删除密钥
    pub fn key_add(&self, paths: &Paths) -> Result<String, String> {
        // 32 字节随机 → 64 位 hex，够长不需要额外哈希（这是本机自用，不是公网服务）
        let mut h = Sha256::new();
        h.update(uuid::Uuid::new_v4().as_bytes());
        h.update(uuid::Uuid::new_v4().as_bytes());
        h.update(now_ms().to_le_bytes());
        let k = format!("zp-{}", hex(&h.finalize())[..32].to_string());
        let mut g = self.inner.lock().unwrap();
        if !g.keys.contains(&k) {
            g.keys.push(k.clone());
        }
        let keys = g.keys.clone();
        drop(g);
        store::set_relay_keys(paths, &keys)?;
        Ok(k)
    }

    pub fn key_del(&self, paths: &Paths, key: &str) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        g.keys.retain(|k| k != key);
        let keys = g.keys.clone();
        drop(g);
        store::set_relay_keys(paths, &keys)
    }

    ///
    /// ⚠ 两个坑（都是实测踩的）：
    /// 1. 端口没变就直接返回 —— 否则白停一次服务，网页当场失联
    /// 2. 新端口起不来要**退回旧端口**，不能把服务留在停掉的状态
    pub fn set_port(&self, paths: &Paths, port: u16) -> Result<Value, String> {
        if port < 1024 {
            return Err("端口要 >= 1024".into());
        }
        let old = self.port_or_default();
        if port == old {
            return Ok(self.console_status(paths));
        }
        // **先记下来**：改端口要重启才生效，不记的话重启就回默认端口，
        // 那句「重启后直接就是新端口」就成了假话。
        if let Err(e) = store::set_relay_port(paths, port) {
            crate::flowlog::log("relay", "port-persist-fail", &e);
        }
        let was_on = self.inner.lock().unwrap().external;
        self.stop();
        // stop() 后旧 listener 有时**并没有真的关掉**（实测：切完新旧端口同时 LISTENING，
        // 再想切回来就被占住、直接 400）。所以这里做行为层确认：旧端口还在应答就别往下走，
        // 免得越切越多监听，最后切不回来。
        for _ in 0..12 {
            if !port_answers(old) {
                break;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        if port_answers(old) {
            self.set_external(was_on);
            crate::flowlog::log("relay", "port-stuck", &format!("旧端口 {old} 停不掉，没敢切"));
            return Err(format!(
                "旧端口 {old} 停不掉（服务其实还在，能正常用）—— 换端口需要**重启一次工具**，重启后直接就是新端口"
            ));
        }
        if let Err(e) = self.start(Paths { home: paths.home.clone() }, port) {
            let _ = self.start(Paths { home: paths.home.clone() }, old);
            self.set_external(was_on);
            return Err(format!("新端口起不来（{e}），已退回 {old}"));
        }
        self.set_external(was_on);
        Ok(self.console_status(paths))
    }

    /// 手动解冻：本地记着「这个号用尽 / 被限流」，但上游可能早就恢复了。
    /// 抄 sub2api 的 reset-quota —— 没有它就只能干等 TTL 到点。
    pub fn unfreeze(&self, id: &str) -> usize {
        let mut g = self.inner.lock().unwrap();
        let before = g.no_quota.len() + g.blocked.len();
        g.no_quota.retain(|(a, _), _| a != id);
        g.blocked.remove(id);
        let n = before - (g.no_quota.len() + g.blocked.len());
        if let Some(h) = g.health.get_mut(id) {
            h.streak = 0;
        }
        n
    }

    pub fn set_external(&self, on: bool) {
        self.inner.lock().unwrap().external = on;
    }

    /// 拿一个验证码。池子空就等取码页现取一个（最多 `PARAM_WAIT_MS`）。
    fn take_param(&self) -> Result<(String, String), String> {
        let t0 = now_ms();
        {
            let mut g = self.inner.lock().unwrap();
            g.waiters += 1;
            // 一有请求就算进入活跃窗口，取码页立刻开始预存
            g.last_use = t0;
        }
        let deadline = t0 + PARAM_WAIT_MS;
        loop {
            {
                let mut g = self.inner.lock().unwrap();
                let now = now_ms();
                let cut = now.saturating_sub(PARAM_MAX_AGE_MS);
                g.params.retain(|p| p.2 >= cut);
                if !g.params.is_empty() {
                    let (param, region, _) = g.params.remove(0);
                    g.last_use = now;
                    g.waiters = g.waiters.saturating_sub(1);
                    return Ok((param, region));
                }
            }
            if now_ms() >= deadline {
                let mut g = self.inner.lock().unwrap();
                g.waiters = g.waiters.saturating_sub(1);
                return Err(crate::i18n::tr("err.gw.no_param"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 取码页交回来一个码。返回池子里的数量。
    fn put_param(&self, param: String, region: String) -> usize {
        let mut g = self.inner.lock().unwrap();
        let now = now_ms();
        g.params.push((param, region, now));
        g.mints += 1;
        g.mint_times.push(now);
        let cut = now.saturating_sub(10 * 60 * 1000);
        g.mint_times.retain(|t| *t >= cut);
        if !g.inflight.is_empty() {
            g.inflight.pop(); // 交回来一单
        }
        g.params.len()
    }

    /// 取码页长轮询：要不要取码。`wait` 时阻塞到确实要取（或超时）才回
    /// —— 页面自己定时问会被浏览器节流（后台标签页一分钟一次）。
    fn want_mint(&self, wait: bool) -> Value {
        let deadline = now_ms() + if wait { PARAM_WAIT_MS } else { 0 };
        loop {
            {
                let mut g = self.inner.lock().unwrap();
                let now = now_ms();
                let cut = now.saturating_sub(PARAM_MAX_AGE_MS);
                g.params.retain(|p| p.2 >= cut);
                g.inflight.retain(|t| now.saturating_sub(*t) < INFLIGHT_TTL_MS);
                let n = g.params.len();
                let active = now.saturating_sub(g.last_use) < ACTIVE_WINDOW_MS;
                let want = g.waiters + if active { PARAM_PRE } else { 0 };
                // ⚠ 在途的单算「供给」不算「需求」。这里把符号写反过一次
                // （写成 + inflight），缺口每派一单反而变大 → 一秒取了 159 个码。
                let mut short = want as i64 - n as i64 - g.inflight.len() as i64;
                if n >= PARAM_MAX_POOL {
                    short = 0; // 兜底：不管逻辑怎么错，池子都不许超过这个数
                }
                if short > 0 {
                    g.inflight.push(now);
                    return json!({ "want": true, "pool": n, "target": n + short as usize });
                }
                if now_ms() >= deadline {
                    return json!({ "want": false, "pool": n, "target": n });
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn start(&self, paths: Paths, port: u16) -> Result<(), String> {
        {
            let g = self.inner.lock().unwrap();
            if g.running {
                return Err(crate::i18n::tr("err.gw.already_running"));
            }
        }
        // ⚠ **socket 永远绑通配地址**，`bind` 只当策略用（见 handle 里的来源判断）。
        //
        // 为什么不用「按 bind 绑具体地址」：实测（2026-09-28）这台机器上
        // 重复绑定同一个端口**是成功的**（旧监听还没释放时新的也能绑上），
        // 于是从 0.0.0.0 切回 127.0.0.1 后同一个进程两个监听同时在跑；
        // 连「先试绑一下看端口空没空」都探不出来（探针永远成功）。
        // 改成永远绑通配 + 应用层判来源：切监听地址不再碰 socket，这类 bug 结构上就不存在了。
        let server = tiny_http::Server::http(("0.0.0.0", port))
            .map_err(|e| crate::i18n::trf("err.gw.bind", &[("e", &e.to_string())]))?;
        let stop = Arc::new(AtomicBool::new(false));
        let serving = Arc::new(AtomicBool::new(true));
        let server = Arc::new(server);
        {
            let mut g = self.inner.lock().unwrap();
            g.running = true;
            g.port = port;
            g.stop = Some(stop.clone());
            g.server = Some(server.clone());
            g.serving = Some(serving.clone());
        }
        let gw = self.clone();
        let server2 = server.clone();
        let stop2 = stop.clone();
        let serving2 = serving.clone();
        std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                match server2.recv_timeout(Duration::from_millis(300)) {
                    Ok(Some(req)) => {
                        let gw = gw.clone();
                        std::thread::spawn(move || {
                            if let Err(e) = handle(req, &gw) {
                                crate::flowlog::log("relay", "req-fail", &e);
                            }
                        });
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
            // 线程真的退到这里 = listener 即将 drop，socket 也就释放了
            serving2.store(false, Ordering::Relaxed);
        });
        let gw2 = self.clone();
        let p2 = paths;
        std::thread::spawn(move || loop {
            let _ = gw2.warm_quota(&p2, 2, |_| {});
            if !gw2.inner.lock().unwrap().running {
                break;
            }
            std::thread::sleep(Duration::from_millis(QUOTA_TTL_MS));
        });
        crate::flowlog::log("relay", "start", &format!("port={port} upstream={UPSTREAM}"));

        // 健康探测线程：每 PROBE_EVERY_MS 探一个号（轮着来 + 抖动）
        let me = self.clone();
        let stop = self.inner.lock().unwrap().stop.clone();
        let halted = move || stop.as_ref().map_or(false, |s| s.load(Ordering::Relaxed));
        std::thread::spawn(move || loop {
            for _ in 0..(PROBE_EVERY_MS / 500) {
                if halted() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            if halted() {
                return;
            }
            // Paths 不实现 Clone，在循环里重新探一次（就是读几个环境变量，很便宜）
            me.probe_tick(&Paths::detect());
        });
        Ok(())
    }

    pub fn stop(&self) {
        let (stop, server, serving, port) = {
            let mut g = self.inner.lock().unwrap();
            let s = g.stop.take();
            let srv = g.server.take();
            let sv = g.serving.take();
            g.running = false;
            (s, srv, sv, g.port)
        };
        let Some(s) = stop else { return };
        s.store(true, Ordering::Relaxed);
        // 光置标志不够：线程阻塞在 recv_timeout 上，不一定会立刻醒。
        // unblock() 让它当场返回，然后我们**等它真的退出**（最多 1 秒）——
        // 这样调用方接着起新监听时，旧 socket 已经释放干净。
        if let Some(srv) = server {
            srv.unblock();
        }
        if let Some(sv) = serving {
            for _ in 0..40 {
                if !sv.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        crate::flowlog::log("relay", "stop", &format!("port={port}"));
    }

    pub fn refresh_quota(&self, paths: &Paths, force: bool) -> Result<usize, String> {
        let mut waited = 0u64;
        loop {
            {
                let mut g = self.inner.lock().unwrap();
                if !g.refreshing {
                    g.refreshing = true;
                    break;
                }
            }
            if waited >= 120_000 {
                return Err(crate::i18n::tr("err.gw.busy_refresh"));
            }
            std::thread::sleep(Duration::from_millis(300));
            waited += 300;
        }

        let accounts: Vec<(String, String)> = store::list_accounts(paths)
            .map(|v| v.into_iter().map(|a| (a.id, a.name)).collect())
            .unwrap_or_default();
        let total = accounts.len();
        let mut n = 0usize;
        let mut failed: Vec<String> = Vec::new();

        for (i, (id, name)) in accounts.iter().enumerate() {
            if !force {
                let g = self.inner.lock().unwrap();
                if let Some((at, _)) = g.quota.get(id) {
                    if now_ms().saturating_sub(*at) < QUOTA_TTL_MS {
                        continue;
                    }
                }
            }
            let mut got = None;
            let mut last = String::new();
            for attempt in 0..4u64 {
                match store::account_quota(paths, id) {
                    Ok(ov) => {
                        got = Some(ov);
                        break;
                    }
                    Err(e) => {
                        last = e;
                        std::thread::sleep(Duration::from_millis(600 + attempt * 700));
                    }
                }
            }
            match got {
                Some(ov) => {
                    self.inner.lock().unwrap().quota.insert(id.clone(), (now_ms(), ov));
                    n += 1;
                }
                None => {
                    let short: String = last.chars().take(140).collect();
                    crate::flowlog::log("relay", "quota-fail", &format!("{name}: {short}"));
                    failed.push(name.clone());
                }
            }
            if i + 1 < total {
                std::thread::sleep(Duration::from_millis(150));
            }
        }

        {
            let mut g = self.inner.lock().unwrap();
            g.refreshing = false;
            g.last_refresh = Some(store::now_ts());
            g.quota_total = total;
            g.last_failed = failed;
        }
        Ok(n)
    }

    fn candidates(&self, paths: &Paths, model: &str) -> Vec<Candidate> {
        let accounts = store::list_accounts(paths).unwrap_or_default();
        let secret = crate::zcrypto::default_secret(&paths.home);
        let (policy, pinned) = {
            let g = self.inner.lock().unwrap();
            (g.policy.clone(), g.pinned.clone())
        };
        let mut list: Vec<Candidate> = vec![];
        let model_k = model_key(model);
        for acc in &accounts {
            if policy == POLICY_PINNED && pinned.as_deref() != Some(acc.id.as_str()) {
                continue;
            }
            // 上游刚说过这个号这个模型用尽 → 别等额度缓存刷新（5 分钟），立刻排除
            {
                let g = self.inner.lock().unwrap();
                if let Some(t) = g.no_quota.get(&(acc.id.clone(), model_k.clone())) {
                    if now_ms().saturating_sub(*t) < NO_QUOTA_TTL_MS {
                        continue;
                    }
                }
            }
            // 风控冷却中的号直接跳过。3012 是**账号级递进惩罚**，撞它只会把惩罚坐实
            {
                let g = self.inner.lock().unwrap();
                if let Some(t) = g.blocked.get(&acc.id) {
                    if *t > now_ms() {
                        continue;
                    }
                }
            }
            let (expire, left) = {
                let g = self.inner.lock().unwrap();
                let Some((_, ov)) = g.quota.get(&acc.id) else { continue };
                let mut best: Option<(Option<String>, f64)> = None;
                for p in &ov.plans {
                    let mut here = 0.0f64;
                    let mut hit = false;
                    for it in &p.items {
                        if model_same(&it.name, model) {
                            let r = it.remaining.unwrap_or(0.0);
                            if r > 0.0 {
                                hit = true;
                                here += r;
                            }
                        }
                    }
                    if !hit {
                        continue;
                    }
                    best = match best {
                        None => Some((p.expire.clone(), here)),
                        Some((be, bl)) => {
                            let sooner = matches!((&p.expire, &be), (Some(x), Some(b)) if x < b)
                                || (p.expire.is_some() && be.is_none());
                            if sooner {
                                Some((p.expire.clone(), here))
                            } else {
                                Some((be, bl + here))
                            }
                        }
                    };
                }
                match best {
                    Some(b) => b,
                    None => continue,
                }
            };
            let Ok(full) = store::load_account(paths, &acc.id) else { continue };
            let Some(jwt) = crate::quota::candidate_tokens(&full.credentials, full.config.as_ref(), &secret)
                .into_iter()
                .next()
            else {
                continue;
            };
            list.push(Candidate {
                id: acc.id.clone(),
                name: acc.name.clone(),
                token: jwt,
                mid: full.virtual_device_mid.unwrap_or_default(),
                expire,
                left,
            });
        }
        match policy.as_str() {
            POLICY_MOST_LEFT => list.sort_by(|a, b| {
                b.left.partial_cmp(&a.left).unwrap_or(std::cmp::Ordering::Equal).then(a.name.cmp(&b.name))
            }),
            _ => list.sort_by(|a, b| a.expire.cmp(&b.expire).then(a.name.cmp(&b.name))),
        }
        list
    }

    fn sticky_order(&self, mut list: Vec<Candidate>) -> Vec<Candidate> {
        if list.is_empty() {
            return list;
        }
        let mut g = self.inner.lock().unwrap();
        if g.policy == POLICY_PINNED {
            return list;
        }
        let cur = g.sticky.clone();
        if let Some(id) = cur {
            if let Some(i) = list.iter().position(|c| c.id == id) {
                list.rotate_left(i);
                return list;
            }
        }
        g.sticky = list.first().map(|c| c.id.clone());
        list
    }

    fn set_sticky(&self, id: &str) {
        let mut g = self.inner.lock().unwrap();
        if g.policy != POLICY_PINNED {
            g.sticky = Some(id.to_string());
        }
    }

}

/// ZCode 当前登录的是哪个**已存档**的号（按 live_hash 匹配）。
/// 中继还没收到客户端请求时用它兜底，不然「当前登录」那张卡一直是空的。
/// 注意：要读 ZCode 的文件，别在持锁时调用。
fn live_account_id(paths: &Paths) -> Option<String> {
    let st = store::get_state(paths).ok()?;
    if !st.live_logged_in {
        return None;
    }
    st.active_account_id
}

fn quota_rows_for(g: &Inner, id: Option<&str>) -> Value {
    let Some(id) = id else { return Value::Null };
    let Some((_, ov)) = g.quota.get(id) else { return Value::Null };
    let mut rows: Vec<Value> = vec![];
    for p in &ov.plans {
        for it in &p.items {
            if it.total.is_none() && it.remaining.is_none() {
                continue;
            }
            rows.push(json!({
                "name": it.name,
                "left": it.remaining,
                "total": it.total,
                "reset": it.reset.clone().or_else(|| it.period_end.clone()),
                "expire": p.expire,
                "plan": p.name,
            }));
        }
    }
    json!({ "account": id, "rows": rows })
}

fn models_seen(quota: &HashMap<String, (u64, crate::quota::QuotaOverview)>) -> Vec<String> {
    let mut set: Vec<String> = vec![];
    for (_, ov) in quota.values() {
        for p in &ov.plans {
            for it in &p.items {
                let n = it.name.trim().to_string();
                if !n.is_empty() && !set.iter().any(|x| model_eq(x, &n)) {
                    set.push(n);
                }
            }
        }
    }
    set.sort();
    set
}

fn peek_model(head: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(head);
    let mut from = 0usize;
    while let Some(rel) = head[from..].find("\"model\"") {
        let i = from + rel + "\"model\"".len();
        let rest = &head[i..];
        let mut it = rest.chars().skip_while(|c| c.is_whitespace());
        if it.next() != Some(':') {
            from = i;
            continue;
        }
        let rest: String = it.collect();
        let rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix('"') {
            if let Some(end) = r.find('"') {
                let m = r[..end].trim().to_string();
                if !m.is_empty() {
                    return Some(m);
                }
            }
        }
        from = i;
    }
    None
}

/// 从响应体里抠业务码：`"code":1005` 或 `"code":"3012"`。
/// 200 的正文里也可能是业务错（额度用尽之类），单看 HTTP 状态不够。
/// 不匹配 `error_code` / `status_code` —— 那些 `code` 前面不是引号。
fn peek_code(head: &[u8]) -> String {
    let s = String::from_utf8_lossy(head);
    let mut from = 0usize;
    while let Some(rel) = s[from..].find("\"code\"") {
        let i = from + rel + "\"code\"".len();
        let rest = &s[i..];
        let mut it = rest.chars().skip_while(|c| c.is_whitespace());
        if it.next() != Some(':') {
            from = i;
            continue;
        }
        let rest: String = it.collect();
        let rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix('"') {
            if let Some(end) = r.find('"') {
                let c = r[..end].trim().to_string();
                if !c.is_empty() {
                    return c;
                }
            }
        } else {
            let c: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !c.is_empty() {
                return c;
            }
        }
        from = i;
    }
    String::new()
}

/// 归一化模型名，用于**额度归属**判断：脱掉 GLM 前缀、去掉连字符，只认完全相等。
///
/// 额度条目叫 `5.3-Flash`（不带 GLM 前缀），所以要脱前缀；但**绝不能用「包含」**——
/// `GLM-5.3` 和 `GLM-5.3-Flash` 是两个独立额度池，互相顶会把号挑去跑它其实没额度的模型，
/// 服务端就回「可用额度已用完」，而中继因为拿到的是 200 不会换号。
fn model_key(s: &str) -> String {
    let flat: String = s
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .collect();
    match flat.strip_prefix("glm") {
        Some(rest) => rest.to_string(),
        None => flat,
    }
}

fn model_same(a: &str, b: &str) -> bool {
    let (x, y) = (model_key(a), model_key(b));
    !x.is_empty() && x == y
}

/// 换号重试前的退避：照客户端 `{ baseDelayMs:2000, backoffFactor:2, maxDelayMs:60000, jitter:true }`。
/// 零延迟连打是自动化特征，客户端从不这么干。
///
/// ⚠ 只用在**对外路径**上 —— 那边每个候选都现取一个新码，重试是安全的。
/// 客户端路径不能重试（客户端的码是一次性的），所以那边不需要退避。
fn backoff_ms(attempt: usize) -> u64 {
    let shift = attempt.saturating_sub(1).min(5) as u32;
    let capped = 2000u64.saturating_mul(1u64 << shift).min(60_000);
    let jitter = (now_ms() % 41) as i64 - 20; // ±20%
    ((capped as i64) * (100 + jitter) / 100).max(50) as u64
}

/// 从 body 的 `metadata.user_id` 里抠出 session_id 的前 8 位。
/// 客户端的 session_id 在**同一个 CLI 会话内是固定的**，所以它一变就说明客户端重启了 ——
/// 用来验证「是不是某一条客户端会话被盯上了」（2026-09-27 的猜想）。
/// 某个号在**某条客户端会话里**该用的 `session_id`。
///
/// 用 `(账号, 客户端会话)` 做确定性哈希 —— **不要按账号固定成一条**。
/// 真人客户端换一条对话就换一个新 sid；按账号固定等于给每个号焊了个**永久指纹**，
/// 一旦上游把它标记了，这个号在**所有**对话里就都废了。
///
/// 实测 2026-09-27 21:42–21:45（用户新开对话照样中）：
/// ```
/// mjyzjq  sent=41cadd50  client_sid=341ef206
/// mjyzjq  sent=41cadd50  client_sid=d1046f49   ← 换了对话，sent 没变 → 又 3012
/// rjsjbf  sent=f1f6fa5d  client_sid=cf0bc12c
/// rjsjbf  sent=f1f6fa5d  client_sid=5f1308c6
/// ```
/// 两对失败里客户端 sid 各不相同，唯一不变的是 `sent=`。**被标的是发出去的那条 sid。**
fn account_session(id: &str, seed: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"zpool-sid|");
    h.update(id.as_bytes());
    h.update(b"|");
    h.update(seed.as_bytes());
    let d = h.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(b).to_string()
}

/// 对外接口没有「客户端会话」可依据，用当天日期当种子：一天内稳定、跨天自动换。
/// 至少不会退化成永久指纹（那正是这一版在客户端路径上修掉的毛病）。
fn day_seed() -> String {
    (now_ms() / 86_400_000).to_string()
}

/// 宽松比对：只用在「要不要改写模型名」「模型列表去重」这类地方，不用于额度计算
fn model_eq(a: &str, b: &str) -> bool {
    let n = |s: &str| s.trim().to_ascii_lowercase().replace(['-', '_', ' '], "");
    let (x, y) = (n(a), n(b));
    !x.is_empty() && !y.is_empty() && (x == y || x.contains(&y) || y.contains(&x))
}

fn body_model(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    v.get("model").and_then(|m| m.as_str()).map(|s| s.to_string())
}

/* ------------------------------------------- 本地接口 / 取码池 / 对外请求 */

/// 数流出去多少字节，顺便把 SSE 里 `usage.*_tokens` 抠出来。
/// 中转站面板上的 token 速度全靠它 —— 不数这个，那个数字就永远是 0。
///
/// 另外它还是**用量落盘的闸门**：流式响应只有读到 `n == 0`（流结束）时才知道
/// 总时长 / 总 token，所以用量记录挂在它身上，读完才写。
struct Meter<R> {
    inner: std::sync::Arc<Mutex<Inner>>,
    src: R,
    tail: Vec<u8>,
    /// 正文最前面一小段，用来抠业务码（错误体很短，`code` 就在开头）
    head: Vec<u8>,
    tokens: u64,
    tokens_in: u64,
    /// 这一发流出去的字节（本地计数 = 差值，天然并发安全）
    bytes: u64,
    /// 这一发请求是什么时候发出去的 —— 用来算「首字节等多久」
    started: u64,
    /// 第一个字节到达的时刻（只记一次）
    first_at: Option<u64>,
    /// 流结束后要落盘的用量记录（ttfb/ms/bytes/tokens/code 收尾时才定）
    rec: Option<crate::usage::UsageRecord>,
    path: std::path::PathBuf,
}

impl<R: Read> Read for Meter<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.src.read(out)?;
        let now = now_ms();
        if n == 0 {
            // 流结束 = 总时长 / 总 token 定下来 → **这时候**才落盘
            if let Some(t) = scan_u64_key(&self.tail, b"input_tokens") {
                self.tokens_in = self.tokens_in.max(t);
            }
            if let Some(t) = scan_u64_after(&self.tail, b"output_tokens") {
                self.tokens = self.tokens.max(t);
            }
            let ms = now.saturating_sub(self.started);
            let ttfb = self.first_at.map(|f| f.saturating_sub(self.started));
            let mut code = peek_code(&self.head);
            if code.is_empty() {
                code = peek_code(&self.tail);
            }
            if let Some(mut rec) = self.rec.take() {
                rec.tokens_in = self.tokens_in;
                rec.tokens_out = self.tokens;
                rec.ttfb = ttfb;
                rec.ms = ms;
                rec.bytes = self.bytes;
                rec.code = code;
                crate::usage::append(&self.path, &rec);
            }
            let mut g = self.inner.lock().unwrap();
            g.last_total_ms = Some(ms);
            return Ok(0);
        }
        // 首个字节：TTFB。它和总时长分开看才有意义 ——
        // 差很大 = 上游久不吐字（上游慢）；两个都长 = 输出量大或网络慢。
        if self.first_at.is_none() {
            self.first_at = Some(now);
            let mut g = self.inner.lock().unwrap();
            g.last_ttfb_ms = Some(now.saturating_sub(self.started));
            g.last_total_ms = None;
        }
        self.bytes += n as u64;
        if self.head.len() < 2048 {
            let room = 2048 - self.head.len();
            self.head.extend_from_slice(&out[..n.min(room)]);
        }
        // usage 可能在 message_delta / message_stop 里，而一个 SSE 事件会被 TCP 分片切开，
        // 所以留一段尾巴拼着找，而不是只在这一个 chunk 里找。
        self.tail.extend_from_slice(&out[..n]);
        if self.tail.len() > 8192 {
            let cut = self.tail.len() - 8192;
            self.tail.drain(..cut);
        }
        if let Some(t) = scan_u64_after(&self.tail, b"output_tokens") {
            self.tokens = self.tokens.max(t);
        }
        if let Some(t) = scan_u64_key(&self.tail, b"input_tokens") {
            self.tokens_in = self.tokens_in.max(t);
        }
        let now = now_ms();
        let mut g = self.inner.lock().unwrap();
        g.bytes_out += n as u64;
        if self.tokens > g.tokens_out {
            g.tokens_out = self.tokens;
        }
        let (bytes, tokens) = (g.bytes_out, g.tokens_out);
        g.meter.push((now, bytes, tokens));
        Ok(n)
    }
}

/// 在字节流里找 `needle` 之后第一个十进制数，取所有出现里的最大值
/// 旧端口是不是**还在应答**。
///
/// 为什么不用「试绑一下看端口空没空」：实测这台机器上重复绑定同一端口会成功
/// （2026-09-28），所以试绑探不出任何东西。**发一个真的 TCP 连接**才准。
fn port_answers(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(300),
    )
    .is_ok()
}

/// 本机在局域网里的那个地址：拿一个 UDP socket「连」一下外部地址，
/// 内核会按路由表选出源地址 —— **不发任何包**，纯本地查询。
/// 用来在页面上显示「局域网地址是多少」。
fn lan_host() -> String {
    use std::net::UdpSocket;
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "<本机IP>".into())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn scan_u64_after(hay: &[u8], needle: &[u8]) -> Option<u64> {
    let mut best: Option<u64> = None;
    let mut i = 0usize;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            let mut j = i + needle.len();
            while j < hay.len() && !hay[j].is_ascii_digit() && j < i + needle.len() + 16 {
                j += 1;
            }
            let mut v: u64 = 0;
            let mut any = false;
            while j < hay.len() && hay[j].is_ascii_digit() {
                v = v.saturating_mul(10).saturating_add((hay[j] - b'0') as u64);
                any = true;
                j += 1;
            }
            if any {
                best = Some(best.map_or(v, |b: u64| b.max(v)));
            }
            i = j;
        } else {
            i += 1;
        }
    }
    best
}

/// 和 `scan_u64_after` 一样找 needle 后面的数字取最大，但**要求 needle 是一个完整键名**：
/// 前一个字节不能是标识符字符。否则 `input_tokens` 会误吞 `cache_read_input_tokens` /
/// `cache_creation_input_tokens`，把 `in` 算成缓存命中量。
fn scan_u64_key(hay: &[u8], needle: &[u8]) -> Option<u64> {
    let mut best: Option<u64> = None;
    let mut i = 0usize;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            let bounded = i == 0 || !(hay[i - 1].is_ascii_alphanumeric() || hay[i - 1] == b'_');
            if bounded {
                let mut j = i + needle.len();
                let lim = (i + needle.len() + 16).min(hay.len());
                while j < lim && !hay[j].is_ascii_digit() {
                    j += 1;
                }
                let mut v: u64 = 0;
                let mut any = false;
                while j < hay.len() && hay[j].is_ascii_digit() {
                    v = v.saturating_mul(10).saturating_add((hay[j] - b'0') as u64);
                    any = true;
                    j += 1;
                }
                if any {
                    best = Some(best.map_or(v, |b: u64| b.max(v)));
                }
            }
            i += needle.len();
        } else {
            i += 1;
        }
    }
    best
}

/// 采样窗口的增量 `(秒, 字节, token)`
fn meter_delta(g: &Inner) -> (f64, u64, u64) {
    let (Some(first), Some(last)) = (g.meter.first(), g.meter.last()) else {
        return (0.0, 0, 0);
    };
    if last.0 <= first.0 {
        return (0.0, 0, 0);
    }
    (
        (last.0 - first.0) as f64 / 1000.0,
        last.1.saturating_sub(first.1),
        last.2.saturating_sub(first.2),
    )
}

/// 某个号的健康快照（抄 sub2api 的 channel monitor，但探测只看额度接口）
#[derive(Clone, Default)]
struct Health {
    ok: u64,
    fail: u64,
    /// 连续失败次数 —— 比总成功率更早暴露「刚坏」
    streak: u32,
    last_ms: Option<u64>,
    last_at: Option<u64>,
    last_err: String,
    /// 近期 `(时刻 ms, 延迟 ms | None)`，用于画延迟趋势
    history: Vec<(u64, Option<u64>)>,
}

impl Health {
    fn json(&self) -> Value {
        let ok_recent = self.history.iter().filter(|(_, m)| m.is_some()).count();
        json!({
            "ok": self.ok,
            "fail": self.fail,
            "streak": self.streak,
            "lastMs": self.last_ms,
            "lastAt": self.last_at,
            "lastErr": self.last_err,
            "rate": if self.history.is_empty() { Value::Null }
                    else { json!((ok_recent as f64 / self.history.len() as f64 * 100.0).round()) },
            "history": self.history.iter().map(|(t, m)| json!({ "at": t, "ms": m })).collect::<Vec<_>>(),
        })
    }
}

/// 取请求里带的凭据：`x-api-key` 或 `Authorization: Bearer xxx`
fn auth_token(req: &tiny_http::Request) -> String {
    for h in req.headers() {
        match h.field.as_str().as_str().to_ascii_lowercase().as_str() {
            "x-api-key" => return h.value.as_str().to_string(),
            "authorization" => {
                let v = h.value.as_str();
                return v.strip_prefix("Bearer ").unwrap_or(v).trim().to_string();
            }
            _ => {}
        }
    }
    String::new()
}

fn mint_stats(g: &Inner) -> Value {
    let now = now_ms();
    let recent = g.mint_times.iter().filter(|t| now.saturating_sub(**t) < 600_000).count();
    json!({
        "pool": g.params.len(),
        "waiters": g.waiters,
        "active": now.saturating_sub(g.last_use) < ACTIVE_WINDOW_MS,
        "mints": g.mints,
        "mints10min": recent,
    })
}

/// 把各家客户端五花八门的拼法归一到 `/v1/messages`：Anthropic SDK 自己会补 `/v1`，
/// 配「完整端点」的客户端可能已经带了。两种都吃，省得让调用方猜。
fn norm_path(url: &str) -> String {
    let p = url.split('?').next().unwrap_or(url);
    let mut s = p.to_string();
    while let Some(rest) = s.strip_prefix("/v1/v1") {
        s = rest.to_string();
    }
    if s.starts_with("/messages") || s.starts_with("/count_tokens") {
        s = format!("/v1{s}");
    }
    s
}

/// 页面可以去哪找。**优先磁盘，找不到才用编译进去的那份** —— 这样改页面只要
/// 刷新浏览器，不用重新打包整个 Rust 二进制。
///
/// 顺序：`ZPOOL_WEB_DIR` 环境变量 → exe 同级的 `web/` → 源码目录的 `assets/`。
/// 最后那条是给开发用的：本机编译出来的二进制里记着编译时的源码路径，那个目录还在
/// 就直接读它。装在别人机器上时那个路径不存在，自然回落到编译进去的版本。
fn web_dirs() -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    if let Ok(d) = std::env::var("ZPOOL_WEB_DIR") {
        if !d.trim().is_empty() {
            v.push(std::path::PathBuf::from(d));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("web"));
        }
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    if src.is_dir() {
        v.push(src);
    }
    v
}

/// 取页面内容：磁盘优先，兜底用内嵌的那份
fn page(name: &str, embedded: &str) -> String {
    for dir in web_dirs() {
        let p = dir.join(name);
        if let Ok(s) = std::fs::read_to_string(&p) {
            crate::flowlog::log("relay", "page-disk", &p.display().to_string());
            return s;
        }
    }
    embedded.to_string()
}

/// 页面响应：带 no-store，否则浏览器缓存住了，改完刷新还看旧的
fn reply_html(req: tiny_http::Request, body: String) -> Result<(), String> {
    let mk = |k: &str, v: &str| tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap();
    let _ = req.respond(tiny_http::Response::new(
        tiny_http::StatusCode(200),
        vec![
            mk("Content-Type", "text/html; charset=utf-8"),
            mk("Cache-Control", "no-store, must-revalidate"),
        ],
        std::io::Cursor::new(body.into_bytes()),
        None,
        None,
    ));
    Ok(())
}

fn reply_json(req: tiny_http::Request, code: u16, v: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(v).unwrap_or_else(|_| b"{}".to_vec());
    let h = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let _ = req.respond(tiny_http::Response::new(
        tiny_http::StatusCode(code),
        vec![h],
        std::io::Cursor::new(body),
        None,
        None,
    ));
    Ok(())
}

/// 下载类响应（导出邮箱库 / 账号库）：`text/plain`，让浏览器直接存成文件。
fn reply_text(req: tiny_http::Request, body: String, filename: &str) -> Result<(), String> {
    let mk = |k: &str, v: &str| tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap();
    let _ = req.respond(tiny_http::Response::new(
        tiny_http::StatusCode(200),
        vec![
            mk("Content-Type", "text/plain; charset=utf-8"),
            mk("Content-Disposition", &format!("attachment; filename=\"{filename}\"")),
            mk("Cache-Control", "no-store"),
        ],
        std::io::Cursor::new(body.into_bytes()),
        None,
        None,
    ));
    Ok(())
}

/// 凭据字段名（小写比较）。命中就掩码 —— 邮箱密码 / refresh_token / JWT / access_token / 密钥。
const SECRET_KEYS: [&str; 12] = [
    "password",
    "refresh_token",
    "refreshtoken",
    "access_token",
    "accesstoken",
    "id_token",
    "jwt",
    "secret",
    "client_secret",
    "api_key",
    "apikey",
    "token",
];

fn is_secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    SECRET_KEYS.iter().any(|m| k == *m || k.ends_with(&format!("_{m}")))
}

/// 掩码：只留前 4 位 + `***`。太短就直接 `***`。
fn mask_secret(s: &str) -> String {
    let n = s.chars().count();
    if n == 0 {
        return String::new();
    }
    if n <= 4 {
        return "***".to_string();
    }
    let head: String = s.chars().take(4).collect();
    format!("{head}***")
}

/// 递归把 JSON 里凭据类字段掩掉。**只有非回环调用才走这里** —— 本机浏览器照旧看到原文。
fn mask_json(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if is_secret_key(k) {
                    if let Some(s) = val.as_str() {
                        *val = json!(mask_secret(s));
                    }
                } else {
                    mask_json(val);
                }
            }
        }
        Value::Array(arr) => {
            for x in arr.iter_mut() {
                mask_json(x);
            }
        }
        _ => {}
    }
}

/// 返回 JSON，但非回环调用时先把凭据字段打码。
fn reply_json_masked(req: tiny_http::Request, code: u16, v: &Value, from_loopback: bool) -> Result<(), String> {
    if from_loopback {
        return reply_json(req, code, v);
    }
    let mut c = v.clone();
    mask_json(&mut c);
    reply_json(req, code, &c)
}

/// 上游风控要看的 system 段（首次成功解析后缓存；失败不缓存，下次请求可重试）。
fn system_blocks(paths: &Paths) -> Result<Vec<Value>, String> {
    static B: std::sync::OnceLock<Vec<Value>> = std::sync::OnceLock::new();
    if let Some(v) = B.get() {
        return Ok(v.clone());
    }
    let v = crate::prompt::system_blocks(paths)?;
    let _ = B.set(v.clone());
    Ok(v)
}

/// 把外部应用的请求改造成 ZCode 的形状。
///
/// **`system` 必须是「ZCode 的在前、调用方的追加在后」** —— 实测前置会 3012
/// （服务端校验的是 system 的开头）。`tools` / `messages` 随便。
///
/// 返回 `(HTTP 状态码, 错误信息)`：请求体本身有问题 → 4xx；取不到 ZCode 的
/// 系统提示词 → 5xx（**绝不能**发一个没有正确 system 的请求出去）。
fn external_body(
    raw: &[u8],
    mid: &str,
    model: &str,
    session: &str,
    paths: &Paths,
) -> Result<Vec<u8>, (u16, String)> {
    let mut v: Value = serde_json::from_slice(raw)
        .map_err(|e| (400, format!("请求体必须是 UTF-8 编码的 JSON：{e}")))?;
    let obj = v
        .as_object_mut()
        .ok_or_else(|| (400, "请求体必须是 JSON 对象".to_string()))?;
    obj.insert("model".into(), json!(model));
    obj.entry("max_tokens").or_insert(json!(8192));

    let caller = obj.get("system").cloned();
    let mut blocks = system_blocks(paths).map_err(|e| (503, e))?;
    match caller {
        Some(Value::String(s)) if !s.trim().is_empty() => {
            blocks.push(json!({ "type": "text", "text": s }))
        }
        Some(Value::Array(a)) => blocks.extend(a.into_iter()),
        _ => {}
    }
    obj.insert("system".into(), Value::Array(blocks));

    // 设备身份用路由到的那个号自己的（服务端按「账号 + 设备」认人）
    let uid = json!({
        "device_id": mid,
        "account_uuid": "",
        "session_id": session,
    })
    .to_string();
    obj.insert("metadata".into(), json!({ "user_id": uid }));
    serde_json::to_vec(&v).map_err(|e| (500, e.to_string()))
}

/// 外部应用（不经过 ZCode 客户端）的模型请求：**补 system + 自己取码 + 换号**。
/// 客户端那条路自带验证码头和 system，走不到这里。
fn handle_external(
    req: tiny_http::Request,
    gw: &Gateway,
    paths: &Paths,
    path: &str,
    body: &[u8],
) -> Result<(), String> {
    let mut slot = Some(req);
    if !gw.inner.lock().unwrap().external {
        return reply_json(
            slot.take().unwrap(),
            403,
            &json!({ "type": "error",
                     "error": { "type": "permission_error",
                                "message": "对外接口没开 —— 到工具的「反代」页打开它" } }),
        );
    }
    let mut model = body_model(body).unwrap_or_else(|| EXTERNAL_MODELS[0].to_string());
    // 用量记录里要留**客户端原本要的**模型名（映射前），所以先存一份
    let client_model = model.clone();
    // 模型映射：客户端要的名字先翻成上游真正的名字，再拿它去选号/发请求。
    // 放在这里是因为**候选是按上游模型算额度的** —— 用客户端的名字选号会选错号。
    let mapped = gw.inner.lock().unwrap().model_map.get(&model).cloned();
    if let Some(to) = mapped.clone() {
        crate::flowlog::log("relay", "model-map", &format!("请求={model} 上游={to}"));
        model = to;
    }
    // 用量落盘的位置（<store_dir>/usage.jsonl）
    let usage_path = crate::usage::path_for(&paths.store_dir());
    // 造一条「没有正文」的用量记录（预检失败 / 全被拒这类）—— ttfb/ms/bytes 留给收尾
    let mk = |acct: &str, status: u16, code: &str, tries: usize, stream: bool, t: u64| {
        crate::usage::UsageRecord {
            t,
            acct: acct.to_string(),
            model: client_model.clone(),
            up: None,
            tokens_in: 0,
            tokens_out: 0,
            ttfb: None,
            ms: 0,
            bytes: 0,
            status,
            code: code.to_string(),
            tries,
            stream,
            mapped: mapped.clone(),
        }
    };
    let target = format!("{UPSTREAM}{path}");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(600))
        .build();

    let cands = gw.sticky_order(gw.candidates(paths, &model));
    if cands.is_empty() {
        // 号池里没有可用于这个模型的号 —— 客户端给了个不存在的模型也会走这里
        let at = now_ms();
        crate::usage::append(&usage_path, &mk("", 502, "no-account", 0, false, at));
        return reply_json(
            slot.take().unwrap(),
            502,
            &json!({ "type": "error",
                     "error": { "type": "overloaded_error",
                                "message": format!("号池里没有可用于 {model} 的号") } }),
        );
    }

    let mut tried = 0usize;
    // 候选回了「200 但不像模型流」时留一份，全部候选都这样就把最后一份原样交给调用方
    let mut last_abnormal: Option<(u16, Option<String>, Vec<u8>, Box<dyn std::io::Read + Send>)> = None;
    // 收尾那几个失败分支要用「最后一发是谁、什么时候发的」
    let mut last_acct = String::new();
    let mut last_at = now_ms();
    for c in cands.iter().take(MAX_TRIES) {
        tried += 1;
        // 验证码是一次性的 → 每个候选都得自己取一个
        let (param, region) = match gw.take_param() {
            Ok(v) => v,
            Err(e) => {
                crate::usage::append(&usage_path, &mk(&c.name, 503, "no-param", tried, false, now_ms()));
                return reply_json(
                    slot.take().unwrap(),
                    503,
                    &json!({ "type": "error", "error": { "type": "api_error", "message": e } }),
                )
            }
        };
        let sess = account_session(&c.id, &day_seed());
        let out_body = match external_body(body, &c.mid, &model, &sess, paths) {
            Ok(b) => b,
            Err((code, e)) => {
                let ty = if code >= 500 {
                    "api_error"
                } else {
                    "invalid_request_error"
                };
                crate::usage::append(&usage_path, &mk(&c.name, code, "bad-body", tried, false, now_ms()));
                return reply_json(
                    slot.take().unwrap(),
                    code,
                    &json!({ "type": "error", "error": { "type": ty, "message": e } }),
                );
            }
        };

        let mut call = agent.request("POST", &target);
        for (k, v) in CLIENT_HEADERS.iter() {
            call = call.set(k, v);
        }
        call = call
            .set("authorization", &format!("Bearer {}", c.token))
            .set("x-api-key", &c.token)
            .set("x-aliyun-captcha-verify-param", &param)
            .set("x-aliyun-captcha-verify-region", &region)
            .set("x-query-id", &uuid::Uuid::now_v7().to_string())
            .set("x-request-id", &uuid::Uuid::new_v4().to_string())
            .set("x-session-id", &uuid::Uuid::new_v4().to_string())
            .set("x-zcode-trace-id", &uuid::Uuid::new_v4().to_string());

        let tried_at = now_ms();
        last_acct = c.name.clone();
        last_at = tried_at;
        let (status, reader, ctype) = match call.send_bytes(&out_body) {
            Ok(r) => {
                let ct = r.header("Content-Type").map(|s| s.to_string());
                (r.status(), r.into_reader(), ct)
            }
            Err(ureq::Error::Status(code, r)) => {
                let ct = r.header("Content-Type").map(|s| s.to_string());
                (code, r.into_reader(), ct)
            }
            Err(e) => {
                crate::flowlog::log("relay", "external-fail", &format!("{} {e}", c.name));
                gw.mark_blocked(&c.id, RATE_TTL_MS);
                std::thread::sleep(Duration::from_millis(backoff_ms(tried)));
                continue;
            }
        };
        if SWITCH_STATUS.contains(&status) {
            if status == 429 {
                gw.mark_blocked(&c.id, RATE_TTL_MS);
            }
            let d = backoff_ms(tried);
            crate::flowlog::log(
                "relay",
                "switch",
                &format!("外部 {} HTTP {status} → 冷却/换下一个（退避 {d}ms）", c.name),
            );
            std::thread::sleep(Duration::from_millis(d));
            continue;
        }
        // 200 里也可能是业务错误（比如「额度已用完」）—— 那不是正常模型流，得换下一个。
        // 只在**确凿**时判异常：流已结束 / 不是 event-stream / 正文带 error，
        // 三者都不成立就照常转发（一次 read 可能只回来半个事件，误判会白烧另一个号的额度）。
        let stream = ctype.as_deref().unwrap_or("").to_ascii_lowercase().contains("event-stream");
        let mut up: Option<String> = None;
        let mut reader: Box<dyn std::io::Read + Send> = Box::new(reader);
        if status == 200 {
            let ct_lower = ctype.as_deref().unwrap_or("").to_ascii_lowercase();
            let mut head: Vec<u8> = Vec::new();
            let mut buf = vec![0u8; 2048];
            let mut ended = false;
            for _ in 0..6 {
                match reader.read(&mut buf) {
                    Ok(0) => { ended = true; break; }
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                    Err(_) => { ended = true; break; }
                }
                if peek_model(&head).is_some() || head.len() >= 4096 { break; }
            }
            // 上游实际用的模型名 = 用量记录里的 `up`
            up = peek_model(&head);
            let txt = String::from_utf8_lossy(&head).to_lowercase();
            if up.is_none()
                && (ended || !ct_lower.contains("event-stream")
                    || txt.contains("\"error\"") || txt.contains("\"code\""))
            {
                let short: String = String::from_utf8_lossy(&head).chars().take(200).collect();
                crate::flowlog::log("relay", "abnormal",
                    &format!("外部 {} ctype={ct_lower} 响应不像模型流，换下一个：{short}", c.name));
                last_abnormal = Some((status, ctype.clone(), head, reader));
                continue;
            }
            reader = Box::new(std::io::Cursor::new(head).chain(reader));
        }
        gw.set_sticky(&c.id);
        {
            let mut g = gw.inner.lock().unwrap();
            g.served += 1;
            if tried > 1 {
                g.switched += 1;
            }
            g.last_route = Some(Route {
                account: c.id.clone(),
                model: model.clone(),
                tries: tried,
            });
        }
        crate::flowlog::log(
            "relay",
            "external-ok",
            &format!(
                "{} HTTP {status} model={model} tries={tried} len={}",
                c.name,
                out_body.len()
            ),
        );

        let mut hs: Vec<tiny_http::Header> = vec![];
        if let Some(ct) = ctype {
            if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], ct.as_bytes()) {
                hs.push(h);
            }
        }
        for (k, v) in [
            ("x-relay-account", c.name.clone()),
            ("x-relay-tries", tried.to_string()),
        ] {
            if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                hs.push(h);
            }
        }
        let reader: Box<dyn std::io::Read + Send> = Box::new(Meter {
            inner: gw.inner.clone(),
            src: reader,
            tail: Vec::new(),
            head: Vec::new(),
            tokens: 0,
            tokens_in: 0,
            bytes: 0,
            started: tried_at,
            first_at: None,
            // ttfb / ms / bytes / in / out / code 都等流读完再填
            rec: Some(crate::usage::UsageRecord {
                t: tried_at,
                acct: c.name.clone(),
                model: client_model.clone(),
                up,
                tokens_in: 0,
                tokens_out: 0,
                ttfb: None,
                ms: 0,
                bytes: 0,
                status,
                code: String::new(),
                tries: tried,
                stream,
                mapped: mapped.clone(),
            }),
            path: usage_path.clone(),
        });
        let resp =
            tiny_http::Response::new(tiny_http::StatusCode(status), hs, reader, None, None);
        let _ = slot.take().unwrap().respond(resp);
        return Ok(());
    }

    if let Some((status, ctype, head, reader)) = last_abnormal {
        let mut hs: Vec<tiny_http::Header> = vec![];
        if let Some(ct) = ctype {
            if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], ct.as_bytes()) {
                hs.push(h);
            }
        }
        // 全部候选都回了「不像模型流」的 200 —— 这也是一发请求，记下来
        let mut rec = mk(
            &last_acct,
            status,
            "abnormal",
            tried,
            hs.iter().any(|h| h.value.as_str().to_ascii_lowercase().contains("event-stream")),
            last_at,
        );
        rec.up = peek_model(&head);
        rec.bytes = head.len() as u64;
        rec.ms = now_ms().saturating_sub(last_at);
        crate::usage::append(&usage_path, &rec);
        let rd: Box<dyn std::io::Read + Send> = Box::new(std::io::Cursor::new(head).chain(reader));
        let resp = tiny_http::Response::new(tiny_http::StatusCode(status), hs, rd, None, None);
        let _ = slot.take().unwrap().respond(resp);
        crate::flowlog::log("relay", "abnormal-all", &format!("外部 tried={tried} path={path}"));
        return Ok(());
    }
    crate::usage::append(&usage_path, &mk(&last_acct, 502, "rejected", tried, false, last_at));
    reply_json(
        slot.take().unwrap(),
        502,
        &json!({ "type": "error",
                 "error": { "type": "overloaded_error",
                            "message": format!("试了 {tried} 个号都被拒") } }),
    )
}

/// 从 URL 的 query 串里取一个参数（值做了简单百分号解码）。
fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split('?').nth(1)?;
    for kv in q.split('&') {
        let (k, v) = match kv.split_once('=') {
            Some((k, v)) => (k, v),
            None => (kv, ""),
        };
        if k == key {
            return Some(decode_pct(v));
        }
    }
    None
}

fn decode_pct(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hexval(b[i + 1]), hexval(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        if b[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hexval(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn handle(mut req: tiny_http::Request, gw: &Gateway) -> Result<(), String> {
    let paths = Paths::detect();
    let url = req.url().to_string();
    let mut body = Vec::new();
    if req.as_reader().read_to_end(&mut body).is_err() {
        let _ = req.respond(tiny_http::Response::from_string("bad body").with_status_code(400));
        return Ok(());
    }


    let path = norm_path(&url);

    // ---- 鉴权：**本机免，外部必须带 key** ----
    // 判断依据是来源地址是不是回环：局域网上来的请求一律要凭据。
    // 这样对话页/控制台（都在本机浏览器里）不用填 key，而 0.0.0.0 暴露出去也拦得住。
    let from_loopback = req
        .remote_addr()
        .map(|a| a.ip().is_loopback())
        .unwrap_or(true);
    // 「仅本机」模式下，外面来的请求一律当不存在（回 404，不暴露这里有个服务）
    if !from_loopback && gw.inner.lock().unwrap().bind.starts_with("127.") {
        crate::flowlog::log("relay", "bind-deny", &format!("{path} 来自非本机，但当前是「仅本机」模式"));
        let _ = req.respond(
            tiny_http::Response::from_string("not found
").with_status_code(404),
        );
        return Ok(());
    }
    if !from_loopback {
        let keys = gw.inner.lock().unwrap().keys.clone();
        if keys.is_empty() {
            // 理论上到不了这儿（set_bind 有护栏），留个兜底别静默放行
            let _ = req.respond(
                tiny_http::Response::from_string("外部访问未启用：没有配置 API Key\n")
                    .with_status_code(401),
            );
            return Ok(());
        }
        let got = auth_token(&req);
        if !keys.iter().any(|k| *k == got) {
            crate::flowlog::log("relay", "auth-deny", &format!("{path} 凭据不对"));
            let _ = req.respond(
                tiny_http::Response::from_string("unauthorized：请在 x-api-key 或 Authorization 里带上 API Key\n")
                    .with_status_code(401),
            );
            return Ok(());
        }
    }

    // ---- 本地页面与接口（对话页 / 控制台 / 取码页 / 模型列表） ----
    match path.as_str() {
        "/" | "/chat" => return reply_html(req, page("chat.html", CHAT_PAGE)),
        "/proxy" | "/proxy/admin" => return reply_html(req, page("proxy.html", PROXY_PAGE)),
        "/proxy/status" => return reply_json(req, 200, &gw.console_status(&paths)),
        // 用量明细：`?limit=&acct=&model=&ok=` 都可选。倒序（最新在前），limit 上限 1000。
        "/proxy/usage" => {
            let limit = query_param(&url, "limit")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(200)
                .min(1000);
            let acct = query_param(&url, "acct").unwrap_or_default().to_lowercase();
            let model = query_param(&url, "model").unwrap_or_default().to_lowercase();
            let ok = query_param(&url, "ok").unwrap_or_default();
            let rows = crate::usage::read_all(&crate::usage::path_for(&paths.store_dir()));
            let total = rows.len();
            let out: Vec<Value> = rows
                .iter()
                .rev()
                .filter(|r| acct.is_empty() || r.acct.to_lowercase().contains(&acct))
                .filter(|r| {
                    model.is_empty()
                        || r.model.to_lowercase().contains(&model)
                        || r.up.as_deref().map(|u| u.to_lowercase().contains(&model)).unwrap_or(false)
                })
                .filter(|r| match ok.as_str() {
                    "1" | "true" | "ok" => !crate::usage::is_fail(r),
                    "0" | "false" | "fail" => crate::usage::is_fail(r),
                    _ => true,
                })
                .take(limit)
                .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
                .collect();
            let count = out.len();
            return reply_json(req, 200, &json!({ "rows": out, "count": count, "total": total }));
        }
        "/proxy/usage/stats" => {
            return reply_json(req, 200, &crate::usage::stats(&crate::usage::path_for(&paths.store_dir())));
        }
        // 日志：真分页。offset 从最新一条往回数，limit 默认 50（上限 500）。
        // 返回 lines + total 让前端自己算页数，别再用滚动条。
        "/proxy/logs" => {
            let offset = query_param(&url, "offset").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
            let limit = query_param(&url, "limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(50).clamp(1, 500);
            let (lines, total) = crate::flowlog::tail_page(offset, limit);
            return reply_json(req, 200, &json!({ "lines": lines, "total": total, "offset": offset, "limit": limit }));
        }
        // 手动探一个 / 全部探一遍
        "/proxy/probe" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let one = v.get("id").and_then(|x| x.as_str()).map(str::to_string);
            let ids: Vec<String> = match one {
                Some(id) => vec![id],
                None => store::list_accounts(&paths).unwrap_or_default().into_iter().map(|a| a.id).collect(),
            };
            let mut res = vec![];
            for id in ids.iter().take(30) {
                let r = gw.probe_one(&paths, id);
                res.push(json!({ "id": id, "ok": r.is_ok(), "ms": r.as_ref().ok(), "err": r.err() }));
            }
            return reply_json(req, 200, &json!({ "results": res, "status": gw.console_status(&paths) }));
        }
        "/proxy/bind" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let bind = v.get("bind").and_then(|x| x.as_str()).unwrap_or("");
            return match gw.set_bind(&paths, bind) {
                Ok(st) => reply_json(req, 200, &st),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/key-add" => {
            return match gw.key_add(&paths) {
                Ok(k) => {
                    crate::flowlog::log("relay", "key-add", "新建了一个 API Key");
                    reply_json(req, 200, &json!({ "key": k, "status": gw.console_status(&paths) }))
                }
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/key-del" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let k = v.get("key").and_then(|x| x.as_str()).unwrap_or("");
            let in_use = !gw.inner.lock().unwrap().bind.starts_with("127.");
            if in_use && gw.inner.lock().unwrap().keys.len() <= 1 {
                return reply_json(req, 400, &json!({ "error": "现在是局域网模式，删掉最后一个 key 会让人进不来（先把监听地址改回 127.0.0.1）" }));
            }
            return match gw.key_del(&paths, k) {
                Ok(_) => reply_json(req, 200, &gw.console_status(&paths)),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/model-map" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let from = v.get("from").and_then(|x| x.as_str()).unwrap_or("");
            let to = v.get("to").and_then(|x| x.as_str()).unwrap_or("");
            return match gw.set_model_map(&paths, from, to) {
                Ok(st) => {
                    crate::flowlog::log("relay", "model-map-set", &format!("{from} -> {}", if to.is_empty() { "(删)" } else { to }));
                    reply_json(req, 200, &st)
                }
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        // 服务级设置（这三样以前只能回 exe 面板点，网页看得见却改不了）
        "/proxy/external" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let on = v.get("on").and_then(|x| x.as_bool()).unwrap_or(false);
            match gw.set_external_and_run(&paths, on) {
                Ok(st) => {
                    crate::flowlog::log("relay", "external-set", &format!("网页把反代设为 {on}"));
                    return reply_json(req, 200, &st);
                }
                Err(e) => return reply_json(req, 400, &json!({ "error": e })),
            }
        }
        "/proxy/port" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let port = v.get("port").and_then(|x| x.as_u64()).unwrap_or(0) as u16;
            return match gw.set_port(&paths, port) {
                Ok(st) => {
                    crate::flowlog::log("relay", "port-set", &format!("网页把端口改成 {port}"));
                    reply_json(req, 200, &st)
                }
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/refresh-quota" => {
            if let Err(e) = gw.refresh_quota(&paths, true) {
                return reply_json(req, 400, &json!({ "error": e }));
            }
            return reply_json(req, 200, &gw.console_status(&paths));
        }
        // 解冻：把这个号的「用尽 / 限流」标记清掉
        "/proxy/unfreeze" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                let n = gw.unfreeze(id);
                crate::flowlog::log("relay", "unfreeze", &format!("{id} 清了 {n} 条标记"));
            }
            return reply_json(req, 200, &gw.console_status(&paths));
        }
        // 选号/模型策略从控制台改 —— 这些是「一边用一边调」的东西，放网页更顺手
        "/proxy/policy" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let policy = v.get("policy").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let pinned = v.get("pinned").and_then(|x| x.as_str()).map(str::to_string);
            if policy.is_empty() {
                return reply_json(req, 400, &json!({ "error": "policy 不能为空" }));
            }
            if let Err(e) = gw.set_policy(policy, pinned) {
                return reply_json(req, 400, &json!({ "error": e }));
            }
            return reply_json(req, 200, &gw.console_status(&paths));
        }
        "/proxy/model-mode" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let mode = v.get("mode").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if let Err(e) = gw.set_model_mode(mode) {
                return reply_json(req, 400, &json!({ "error": e }));
            }
            return reply_json(req, 200, &gw.console_status(&paths));
        }
        // 页面把报错回传过来，不用开 F12 也能在 oauth.log 里看到
        "/client-log" => {
            if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("?");
                let detail = v.get("detail").and_then(|x| x.as_str()).unwrap_or("");
                crate::flowlog::log("chat", kind, &detail.chars().take(300).collect::<String>());
            }
            return reply_json(req, 200, &json!({ "ok": true }));
        }
        "/mint" => return reply_html(req, page("mint.html", MINT_PAGE)),
        "/want-mint" => {
            let v = gw.want_mint(url.contains("wait"));
            return reply_json(req, 200, &v);
        }
        "/mint-result" => {
            if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                let param = v.get("param").and_then(|p| p.as_str()).unwrap_or("").trim().to_string();
                let region = v
                    .get("region")
                    .and_then(|p| p.as_str())
                    .unwrap_or(CAPTCHA_REGION)
                    .to_string();
                if !param.is_empty() {
                    let cid = v.get("certifyId").and_then(|p| p.as_str()).unwrap_or("?").to_string();
                    let n = gw.put_param(param, region);
                    crate::flowlog::log("relay", "mint", &format!("+1 池={n} certifyId={cid}"));
                }
            }
            return reply_json(req, 200, &json!({ "ok": true }));
        }
        "/v1/models" | "/models" => {
            let data: Vec<Value> = EXTERNAL_MODELS
                .iter()
                .map(|m| json!({ "id": m, "type": "model", "object": "model" }))
                .collect();
            return reply_json(req, 200, &json!({ "object": "list", "data": data }));
        }
        // ---- 管理接口：把 exe 面板的「数据操作」搬进网页 ----
        // 业务逻辑一律复用 store:: / pool:: / claim:: 现成的函数，不重写一遍。
        // 有副作用（切换/重启/杀进程）的由前端二次确认。
        // 凭据字段非回环一律打码（reply_json_masked）。
        "/proxy/accounts" => {
            return match store::get_state(&paths) {
                Ok(st) => reply_json(req, 200, &json!(st)),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/account/quota" => {
            let id = query_param(&url, "id").unwrap_or_default();
            return match store::account_quota(&paths, &id) {
                Ok(q) => reply_json(req, 200, &json!(q)),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/account/capture" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let name = v.get("name").and_then(|x| x.as_str()).map(str::to_string).filter(|s| !s.trim().is_empty());
            let r = { let _g = crate::store_guard(); store::capture_current(&paths, name) };
            return match r {
                Ok(a) => { gw.sync_exe(); reply_json_masked(req, 200, &json!(a), from_loopback) }
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/account/rename" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let r = { let _g = crate::store_guard(); store::rename_account(&paths, &id, &name) };
            return match r {
                Ok(a) => { gw.sync_exe(); reply_json_masked(req, 200, &json!(a), from_loopback) }
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/account/delete" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let r = { let _g = crate::store_guard(); store::delete_account(&paths, &id) };
            return match r {
                Ok(()) => { gw.sync_exe(); reply_json(req, 200, &json!({ "ok": true, "status": gw.console_status(&paths) })) }
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/account/update-live" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let r = { let _g = crate::store_guard(); store::update_account_from_live(&paths, &id) };
            return match r {
                Ok(a) => { gw.sync_exe(); reply_json_masked(req, 200, &json!(a), from_loopback) }
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        // ⚠ 有副作用：会关掉并重启 ZCode
        "/proxy/account/switch" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let force = v.get("force").and_then(|x| x.as_bool()).unwrap_or(false);
            let restart = v.get("restart").and_then(|x| x.as_bool()).unwrap_or(true);
            let r = { let _g = crate::store_guard(); store::switch_to(&paths, &id, force, restart) };
            return match r {
                Ok(s) => { gw.sync_exe(); reply_json(req, 200, &json!({ "result": s, "status": gw.console_status(&paths) })) }
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        // 领取套餐三步：预览 / 激活刷新 / 真正提交。
        // 提交要弹 exe 的滑块验证码窗 —— 靠 setup 塞进来的 AppHandle 把窗拉起来，
        // 用户在原生窗里滑完，结果落在 claim_captcha_submit 里，网页轮询 claim-result 取。
        "/proxy/account/claim-preview" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let mid = match store::account_mid(&paths, &id) { Ok(m) => m, Err(e) => return reply_json(req, 400, &json!({ "error": e })) };
            let acc = match store::load_account(&paths, &id) { Ok(a) => a, Err(e) => return reply_json(req, 400, &json!({ "error": e })) };
            return match crate::claim::preview_plans(&paths.home, &acc.credentials, acc.config.as_ref(), Some(mid)) {
                Ok(plans) => reply_json(req, 200, &json!({ "plans": plans })),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/account/claim-refresh" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let mid = match store::account_mid(&paths, &id) { Ok(m) => m, Err(e) => return reply_json(req, 400, &json!({ "error": e })) };
            let acc = match store::load_account(&paths, &id) { Ok(a) => a, Err(e) => return reply_json(req, 400, &json!({ "error": e })) };
            let (activated, activation_error) = match crate::claim::telemetry_user_id(&paths.home, &acc.credentials) {
                Some(uid) => match crate::claim::report_activation_events(&uid, &mid) {
                    Ok(()) => (true, None),
                    Err(e) => (false, Some(e)),
                },
                None => (false, None),
            };
            return match crate::claim::preview_plans(&paths.home, &acc.credentials, acc.config.as_ref(), Some(mid)) {
                Ok(plans) => reply_json(req, 200, &json!({ "plans": plans, "activated": activated, "activationError": activation_error })),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        // 真正提交领取：把 exe 的滑块验证码窗拉起来，用户在原生窗里滑。
        // 提交本身由 captcha.js 走 claim_captcha_submit 完成，网页这边只负责开窗 + 轮询结果。
        "/proxy/account/claim-start" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let plan_id = v.get("plan_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let auto = v.get("auto").and_then(|x| x.as_bool()).unwrap_or(false);
            let Some(app) = gw.app() else {
                return reply_json(req, 400, &json!({ "error": "反代还没拿到 AppHandle（窗口环境未就绪），这一步暂时只能在 exe 面板做" }));
            };
            return match crate::proxy_claim_start(&app, id, plan_id, auto) {
                Ok(v) => reply_json(req, 200, &v),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        // 领取结果：captcha 窗提交后落在这里（含 accountId / 成功或失败原因），网页轮询
        "/proxy/account/claim-result" => {
            return reply_json(req, 200, &crate::claim_last_result());
        }
        // ZCode 进程 / 路径：文件对话框浏览器给不了，这里只显示 + 手填提交
        "/proxy/zcode/launch" => {
            let (p, ok) = store::effective_zcode_path(&paths);
            if !ok {
                return reply_json(req, 400, &json!({ "error": format!("ZCode 路径无效：{p}") }));
            }
            return match store::launch_zcode(&p) {
                Ok(()) => reply_json(req, 200, &json!({ "ok": true, "status": gw.console_status(&paths) })),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/zcode/kill" => {
            return match store::kill_zcode() {
                Ok(_) => reply_json(req, 200, &json!({ "ok": true, "status": gw.console_status(&paths) })),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/zcode/path" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let _g = crate::store_guard();
            let mut s = store::load_settings(&paths);
            s.zcode_path = store::normalize_zcode_path(&path);
            let r = store::save_settings(&paths, &s);
            drop(_g);
            return match r {
                Ok(()) => reply_json(req, 200, &json!({ "ok": true })),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/accounts/export" => {
            let accounts = match store::list_accounts(&paths) { Ok(a) => a, Err(e) => return reply_json(req, 400, &json!({ "error": e })) };
            if accounts.is_empty() {
                return reply_json(req, 400, &json!({ "error": "账号库是空的" }));
            }
            let mut payload = store::export_bundle_value(&accounts);
            if !from_loopback { mask_json(&mut payload); }
            let body = serde_json::to_string_pretty(&payload).unwrap_or_default() + "\n";
            return reply_text(req, body, "zcode-accounts.json");
        }
        // ---- 邮箱库（mail.json） ----
        "/proxy/mail" => {
            let root = paths.store_dir();
            let items: Vec<_> = crate::pool::load(&root).iter().map(|a| a.to_summary()).collect();
            return reply_json(req, 200, &json!({ "accounts": items }));
        }
        "/proxy/mail/get" => {
            let email = query_param(&url, "email").unwrap_or_default();
            let root = paths.store_dir();
            let accounts = crate::pool::load(&root);
            return match crate::pool::find(&accounts, &email) {
                Some(a) => reply_json_masked(req, 200, &json!({ "email": a.email, "password": a.password }), from_loopback),
                None => reply_json(req, 404, &json!({ "error": "邮箱不在库里" })),
            };
        }
        // 导入：接收浏览器上传的文本内容（不再走系统文件选择框）
        "/proxy/mail/import" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let raw = v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let root = paths.store_dir();
            let r = {
                let _g = crate::store_guard();
                let mut accounts = crate::pool::load(&root);
                let rep = crate::pool::import(&mut accounts, &raw, store::now_ts());
                match crate::pool::save(&root, &accounts) { Ok(()) => Ok(rep), Err(e) => Err(e) }
            };
            return match r {
                Ok(rep) => reply_json(req, 200, &json!({ "added": rep.added, "skipped": rep.skipped, "parsed": rep.total_parsed })),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/mail/remove" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let email = v.get("email").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let root = paths.store_dir();
            let r = {
                let _g = crate::store_guard();
                let mut accounts = crate::pool::load(&root);
                let before = accounts.len();
                accounts.retain(|a| !a.email.eq_ignore_ascii_case(email.trim()));
                let removed = before - accounts.len();
                match crate::pool::save(&root, &accounts) { Ok(()) => Ok(removed), Err(e) => Err(e) }
            };
            return match r {
                Ok(n) => reply_json(req, 200, &json!({ "removed": n })),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/mail/remove-many" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let emails: Vec<String> = v.get("emails").and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_ascii_lowercase()).collect())
                .unwrap_or_default();
            let root = paths.store_dir();
            let r = {
                let _g = crate::store_guard();
                let mut accounts = crate::pool::load(&root);
                let before = accounts.len();
                accounts.retain(|a| !emails.contains(&a.email.trim().to_ascii_lowercase()));
                let removed = before - accounts.len();
                match crate::pool::save(&root, &accounts) { Ok(()) => Ok(removed), Err(e) => Err(e) }
            };
            return match r {
                Ok(n) => reply_json(req, 200, &json!({ "removed": n })),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/mail/mark-verified" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let email = v.get("email").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(true);
            let note = v.get("note").and_then(|x| x.as_str()).map(str::to_string);
            let root = paths.store_dir();
            let r = {
                let _g = crate::store_guard();
                let mut accounts = crate::pool::load(&root);
                if let Some(a) = crate::pool::find_mut(&mut accounts, &email) {
                    if ok {
                        a.status = crate::pool::STATUS_VERIFIED.to_string();
                        a.verified_at = Some(store::now_ts());
                        a.note = None;
                    } else {
                        a.status = crate::pool::STATUS_FAILED.to_string();
                        a.note = note;
                    }
                    crate::pool::save(&root, &accounts)
                } else {
                    Ok(())
                }
            };
            return match r {
                Ok(()) => reply_json(req, 200, &json!({ "ok": true })),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/mail/reset" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let email = v.get("email").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let root = paths.store_dir();
            let r = { let _g = crate::store_guard(); crate::pool::set_status(&root, &email, crate::pool::STATUS_NEW, None) };
            return match r {
                Ok(()) => reply_json(req, 200, &json!({ "ok": true })),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/mail/upsert" => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
            let line = v.get("line").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let Some((email, password, client_id, refresh_token)) = crate::pool::parse_lines(&line).into_iter().next() else {
                return reply_json(req, 400, &json!({ "error": "行格式不对，应为 email----password----client_id----refresh_token" }));
            };
            let root = paths.store_dir();
            let r = {
                let _g = crate::store_guard();
                let mut accounts = crate::pool::load(&root);
                let updated = if let Some(a) = crate::pool::find_mut(&mut accounts, &email) {
                    a.password = password;
                    a.client_id = client_id;
                    a.refresh_token = refresh_token;
                    a.status = crate::pool::STATUS_NEW.to_string();
                    a.note = None;
                    true
                } else {
                    accounts.push(crate::pool::MailAccount {
                        email: email.clone(),
                        password,
                        client_id,
                        refresh_token,
                        status: crate::pool::STATUS_NEW.to_string(),
                        verified_at: None,
                        note: None,
                        created_at: store::now_ts(),
                    });
                    false
                };
                match crate::pool::save(&root, &accounts) { Ok(()) => Ok(updated), Err(e) => Err(e) }
            };
            return match r {
                Ok(updated) => reply_json(req, 200, &json!({ "email": email, "updated": updated })),
                Err(e) => reply_json(req, 400, &json!({ "error": e })),
            };
        }
        "/proxy/mail/export" => {
            let root = paths.store_dir();
            let accounts = crate::pool::load(&root);
            if accounts.is_empty() {
                return reply_json(req, 400, &json!({ "error": "邮箱库是空的" }));
            }
            let body: String = if from_loopback {
                accounts.iter()
                    .map(|a| format!("{}----{}----{}----{}\n", a.email, a.password, a.client_id, a.refresh_token))
                    .collect()
            } else {
                // 非回环：导出行里的凭据也打码，别让局域网把密码/token 整份拉走
                accounts.iter()
                    .map(|a| format!("{}----{}----{}----{}\n", a.email, mask_secret(&a.password), a.client_id, mask_secret(&a.refresh_token)))
                    .collect()
            };
            return reply_text(req, body, "mailboxes.txt");
        }
        // 邮箱管理：看收件箱正文（复用 graph，顺手刷新 refresh_token）
        "/proxy/mail/messages" => {
            let email = query_param(&url, "email").unwrap_or_default();
            let top = query_param(&url, "top").and_then(|v| v.parse::<usize>().ok()).unwrap_or(15);
            let root = paths.store_dir();
            let accounts = crate::pool::load(&root);
            let acc = match crate::pool::find(&accounts, &email) {
                Some(a) => a.clone(),
                None => return reply_json(req, 404, &json!({ "error": "邮箱不在库里" })),
            };
            let (msgs, new_rt) = match crate::graph::fetch_messages(&acc.client_id, &acc.refresh_token, top) {
                Ok(v) => v,
                Err(e) => {
                    if crate::graph::is_credential_error(&e) {
                        let _ = crate::pool::set_status(&root, &email, crate::pool::STATUS_INVALID, Some("需要重新授权".to_string()));
                    }
                    return reply_json(req, 400, &json!({ "error": e }));
                }
            };
            if let Some(rt) = new_rt {
                let _g = crate::store_guard();
                let mut accounts = crate::pool::load(&root);
                if let Some(a) = crate::pool::find_mut(&mut accounts, &email) {
                    a.refresh_token = rt;
                    let _ = crate::pool::save(&root, &accounts);
                }
            }
            // 原文 html + 纯文本兜底 + 抠出来的链接，前端在沙箱 iframe 里渲染原文，
            // 链接单列出来点/复制（取激活链接用）。`body` 保留成 text 兼容老前端。
            let out: Vec<Value> = msgs.iter().map(|m| json!({
                "id": m.id, "subject": m.subject, "from": m.from,
                "receivedAt": m.received_at, "preview": m.preview,
                "html": m.html, "text": m.text, "body": m.text,
                "links": m.links,
            })).collect();
            return reply_json(req, 200, &json!({ "messages": out, "count": out.len() }));
        }
        _ => {}
    }

    // ---- 外部应用：替它补 system prompt 和验证码 ----
    if path.starts_with("/v1") {
        return handle_external(req, gw, &paths, &path, &body);
    }

    // ⚠ 没匹配上的路径**必须**回一个响应。之前拆掉客户端路径后这里是裸的 Ok(())，
    // 连接被晾着不回复，浏览器只会报「打不开」，看不出是 404 还是服务挂了。
    let _ = req.respond(
        tiny_http::Response::from_string("404 — / 是对话页，/proxy 是控制台\n")
            .with_status_code(404),
    );
    Ok(())
}
