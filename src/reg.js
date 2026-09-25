/*
 * 注册助手（主窗侧）。
 *
 * 登录窗口里跑的是注入驱动（src-tauri/src/driver/zpool-driver.js），它只回传
 * 「码 + 动态参数」，这里负责把码翻成当前语言的文案，并渲染：
 *   - 抽屉：阶段进度、人工介入提示条、过程日志；
 *   - 输入弹窗：名称 / 邮箱 / 密码，或邮箱里的激活链接。
 *
 * 抽屉挂在 body 上而不是 #app 里：主窗刷新会整块重写 #app.innerHTML，
 * 助手状态不该跟着被清掉。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { t, has, stripErr } from "./i18n.js";
import { ic } from "./icons.js";
import { esc, toast } from "./ui.js";

const STEPS = {
  register: [
    "open-signup", "signup-form", "await-email", "open-link",
    "complete-signup", "back-to-authorize", "confirm-authorize", "done",
  ],
  login: ["confirm-authorize", "done"],
  observe: [],
};

const PAGE_KEYS = {
  "zai-signup": "signup",
  "zai-auth": "zaiAuth",
  "zai-authorize": "zaiAuthorize",
  "zai-other": "zaiOther",
  "bigmodel-login": "bigmodel",
  "zcode-bridge": "bridge",
  other: "other",
};

const MAX_LOGS = 200;

/*
 * 自动取链：注册提交后，反复问邮箱池（直连微软 Graph）要「最新一封
 * 邮件里的激活链接」，命中就经 reg_input 下发给驱动。邮件有延迟，所以轮询。
 * 工具没配 / 连不上 / 超时 → 回落到手动粘贴弹窗，不把流程卡死。
 */
const LINK_POLL_MS = 3000;
const LINK_POLL_TRIES = 30; // ≈ 90s
/** 同一账号最多自动插入几次：驱动反复来要，说明前一条没生效，超过就交回人手 */
const LINK_MAX_AUTO = 3;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let S = null;
let drawer = null;
let modal = null;
/** 当前弹窗类型；用来避免对同一种 ask 反复弹框 */
let modalKind = null;
/** 正在轮询取链；防止驱动重复 ask 时并发起多轮 */
let linkBusy = false;

/** 批量模式下「跳过当前账号」的回调，由 main.js 接到邮箱库的队列上 */
let onSkip = null;
export function setRegSkip(fn) {
  onSkip = fn;
}

function tr(group, key, params) {
  const k = `${group}.${key}`;
  return has(k) ? t(k, params) : `${key}`;
}

function nowLabel() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

function stepsFor(mode) {
  return STEPS[mode] || STEPS.login;
}

// ---------------------------------------------------------------- 会话

export function regActive() {
  return !!S;
}

export function regStart(mode, provider) {
  S = {
    mode,
    provider,
    phase: "",
    note: "",
    logs: [],
    closed: false,
    failed: false,
    startedAt: Date.now(),
    /** 注册用的邮箱：自动取激活链接靠它去邮箱里查 */
    signupEmail: "",
    /** 批量验证时当前正在跑的邮箱；非空即批量模式，助手要什么后端直接答 */
    batchEmail: "",
    /** 已经自动下发过几次链接 */
    linkAuto: 0,
  };
  linkBusy = false;
  pushLog("info", "connected", provider);
  render();
}

/** 批量验证：告诉助手当前在处理哪个邮箱（空串退出批量模式）。 */
export function regBatchSet(email) {
  if (S) S.batchEmail = email || "";
}

export function regClose() {
  closeModal();
  S = null;
  document.body.classList.remove("reg-open");
  if (drawer) {
    drawer.remove();
    drawer = null;
  }
}

export function regEvent(ev) {
  if (!S || !ev || typeof ev !== "object") return;
  switch (ev.kind) {
    case "ready":
      pushLog("info", "connected", ev.provider || S.provider);
      break;
    case "page":
      pushLog("info", "page", tr("reg.page", PAGE_KEYS[ev.page] || "other"));
      break;
    case "phase":
      S.phase = ev.phase;
      if (ev.phase === "unsupported") S.failed = true;
      break;
    case "note":
      S.note = !ev.code || ev.code === "clear" ? "" : tr("reg.note", ev.code, { a: ev.a });
      break;
    case "ask":
      // 批量验证：助手要什么，后端直接答，不弹任何窗
      if (S.batchEmail) {
        if (ev.ask === "signup") autoSignup();
        else autoFetchLink();
      } else if (ev.ask === "link") {
        // 激活链接不再让人去邮箱里翻：自动取最新那封里的链接。
        autoFetchLink();
      } else {
        openAsk(ev.ask);
      }
      break;
    case "log":
      pushLog(ev.level || "info", ev.code, ev.a, ev.b);
      break;
    case "closed":
      S.closed = true;
      S.note = "";
      pushLog("warn", "windowClosed");
      break;
    default:
      return;
  }
  render();
}

function pushLog(level, code, a, b) {
  if (!S) return;
  const text = tr("reg.log", code, { a: a || "", b: b || "" });
  S.logs.push({ level, text, at: nowLabel() });
  if (S.logs.length > MAX_LOGS) S.logs.splice(0, S.logs.length - MAX_LOGS);
}

// ---------------------------------------------------------------- 批量验证

/** 批量验证：从邮箱池取密码，自动填注册表单（不弹窗）。 */
async function autoSignup() {
  const email = S && S.batchEmail;
  if (!email) return;
  try {
    const r = await invoke("pool_get", { email });
    S.signupEmail = r.email;
    await invoke("reg_input", {
      ask: "signup",
      value: { name: randName(), email: r.email, password: r.password },
    });
    pushLog("info", "batchSignup", r.email);
  } catch (e) {
    pushLog("warn", "batchSignupFail", stripErr(e));
  }
  render();
}

// ---------------------------------------------------------------- 自动取激活链接

/**
 * 从候选链接里挑一条验证链接。
 *
 * 不能无脑取第一条：注册成功后 Z.ai 会追发一封「欢迎加入」邮件，它比验证邮件还新，
 * 里面的链接是普通的 /auth，不是验证链接（真实抓包：欢迎邮件 23:38 晚于验证邮件 23:37）。
 * 所以优先挑带验证特征的，都没有才退回最新那条。
 */
function pickLink(links) {
  const verify = links.find((u) => /verify|token=|confirm|activ|signup/i.test(u));
  return verify || links[0] || "";
}

/**
 * 注册已提交 → 反复问本机邮箱工具要激活链接。
 *
 * 「驱动不持有收信通道」这条边界不变：取信在宿主（这里）做，取到的链接再经
 * reg_input 下发，驱动那边照旧只认「链接到位就打开」。取不到就回落手动粘贴。
 */
async function autoFetchLink() {
  if (!S || linkBusy) return;
  const email = S.signupEmail;
  if (!email) return startManualLink("noEmail");
  if (S.linkAuto >= LINK_MAX_AUTO) return startManualLink("loop");

  linkBusy = true;
  pushLog("info", "fetchLinkStart", email);
  render();

  let link = "";
  let fail = "";
  // 取不到时用来判断到底是「邮件没到」还是「到了但没有链接」。
  // 后端会回传最新一封的主题，直接甩进提示里，省得靠猜。
  let info = "";
  for (let i = 0; i < LINK_POLL_TRIES && !S.closed; i++) {
    try {
      const r = await invoke("reg_fetch_link", { email, limit: 15 });
      const links = (r && r.links) || [];
      if (links.length) {
        link = pickLink(links);
        break;
      }
      const n = Number(r && r.scanned) || 0;
      const subj = String((r && r.newestSubject) || "").trim();
      info = subj ? t("reg.linkNewest", { n, s: subj }) : t("reg.linkEmptyBox", { n });
      fail = "";
    } catch (e) {
      // 邮箱工具没配 / 连不上 / 账号不存在 —— 别空转 90 秒，直接交回人手
      fail = stripErr(e);
      pushLog("warn", "fetchLinkErr", fail);
      break;
    }
    // 每次轮询都报进度。以前是「第一次 + 每 15 秒一条」，用户容易在静默期
    // 以为卡死了就取消 —— 而验证邮件正常要 8~15 秒才到。
    pushLog("info", "fetchLinkPolling", String(Math.round(((i + 1) * LINK_POLL_MS) / 1000)), String(i + 1));
    render();
    await sleep(LINK_POLL_MS);
  }
  linkBusy = false;

  if (!link) return startManualLink(fail || info || "timeout");

  S.linkAuto += 1;
  try {
    await invoke("reg_input", { ask: "link", value: link });
    pushLog("ok", "fetchLinkFound", link);
  } catch (e) {
    pushLog("warn", "fetchLinkPushFail", stripErr(e));
    startManualLink("pushFail");
  }
  render();
}

/** 自动取链走不通时的兜底：弹手动粘贴框，人自己从邮箱复制。 */
function startManualLink(reason) {
  if (!S) return;
  if (modalKind === "link") return; // 已经在让手动粘贴了，别反复弹
  if (reason) pushLog("warn", "fetchLinkManual", reason);
  render();
  openAsk("link");
}

// ---------------------------------------------------------------- 渲染

function modeLabel(mode) {
  return tr("reg.mode", mode);
}

function ensureDrawer() {
  if (!drawer) {
    drawer = document.createElement("aside");
    drawer.className = "drw";
    document.body.appendChild(drawer);
  }
  return drawer;
}

function statusOf() {
  if (S.closed) return { key: "closed", cls: "off" };
  if (S.failed) return { key: "attention", cls: "warn" };
  if (S.phase === "done") return { key: "done", cls: "ok" };
  return { key: "live", cls: "run" };
}

function stepState(step) {
  const list = stepsFor(S.mode);
  const cur = list.indexOf(S.phase);
  const idx = list.indexOf(step);
  if (cur < 0) return "";
  if (idx < cur) return "done";
  if (idx === cur) return "cur";
  return "";
}

function render() {
  if (!S) return;
  const root = ensureDrawer();
  document.body.classList.add("reg-open");
  const list = stepsFor(S.mode);
  const status = statusOf();

  const stepsHtml = list.length
    ? `<div class="drw-steps">${list.map((s) => `
        <span class="drw-step ${stepState(s)}"><i></i>${esc(tr("reg.step", s))}</span>`).join("")}</div>`
    : "";

  const logsHtml = S.logs.map((l) => `
    <div class="drw-line ${esc(l.level)}">
      <span class="at">${esc(l.at)}</span>
      <span class="tx">${esc(l.text)}</span>
    </div>`).join("");

  root.innerHTML = `
    <div class="drw-h">
      <span class="dot ${status.cls}"></span>
      <span class="drw-title">${esc(t("reg.title"))} · ${esc(modeLabel(S.mode))}</span>
      <span class="chip new">${esc(S.provider || "")}</span>
      <span class="spacer"></span>
      <span class="drw-status ${status.cls}">${esc(tr("reg.status", status.key))}</span>
      <button class="iconbtn" data-act="close" title="${esc(t("reg.btn.close"))}" style="color:var(--ink-2)">${ic("x", 14)}</button>
    </div>
    <div class="bar" style="border-bottom:1px solid var(--line);padding:9px 14px;gap:6px">
      ${S.mode === "register" ? "" : `<button class="btn sm" data-act="to-register">${ic("userPlus", 12)} ${esc(t("reg.btn.toRegister"))}</button>`}
      <button class="btn sm" data-act="reveal">${ic("play", 12)} ${esc(t("reg.btn.reveal"))}</button>
      <button class="btn sm" data-act="retry">${ic("refresh", 12)} ${esc(t("reg.btn.retry"))}</button>
      ${S.batchEmail ? `<button class="btn sm" data-act="skip">${ic("x", 12)} ${esc(t("reg.btn.skip"))}</button>` : ""}
    </div>
    ${S.note ? `<div class="drw-note">${ic("alert", 14)}<span>${esc(S.note)}</span></div>` : ""}
    ${stepsHtml}
    <div class="drw-log" id="reg-log">${logsHtml || `<div class="drw-line"><span class="tx" style="color:var(--ink-3)">${esc(t("reg.waiting"))}</span></div>`}</div>
    ${S.closed ? `<div style="padding:14px"><button class="btn g" data-act="close">${esc(t("reg.btn.close"))}</button></div>` : ""}
  `;

  const log = root.querySelector("#reg-log");
  if (log) log.scrollTop = log.scrollHeight;

  root.querySelectorAll("[data-act]").forEach((b) => {
    b.addEventListener("click", () => onAction(b.dataset.act));
  });
}

async function onAction(act) {
  if (act === "close") return regClose();
  try {
    if (act === "reveal") await invoke("reg_action", { action: "reveal" });
    else if (act === "retry") {
      S.failed = false;
      await invoke("reg_action", { action: "retry" });
    } else if (act === "skip") {
      if (onSkip) onSkip();
      return; // 队列已经往前走了，这个抽屉马上会被关掉，不用重画
    } else if (act === "to-register") {
      const m = await invoke("reg_set_mode", { mode: "register" });
      S.mode = m;
      S.note = "";
      S.logs.push({ level: "info", text: tr("reg.log", "modeSwitch", { a: modeLabel(m) }), at: nowLabel() });
    }
  } catch (e) {
    toast(String(e).replace(/^[a-z_]+:/, ""), "err");
  }
  render();
}

// ---------------------------------------------------------------- 输入弹窗

function closeModal() {
  if (modal) {
    modal.remove();
    modal = null;
  }
  modalKind = null;
}

function field(id, label, type, placeholder, extra = "") {
  return `
    <label class="fld">
      <span>${esc(label)}</span>
      <input id="${id}" class="inp" type="${type}" placeholder="${esc(placeholder)}"
        autocomplete="off" spellcheck="false" ${extra}>
    </label>`;
}

// 注册昵称：随机 6 位小写字母，省得每次手打
function randName() {
  const A = "abcdefghijklmnopqrstuvwxyz";
  let s = "";
  for (let i = 0; i < 6; i++) s += A[Math.floor(Math.random() * A.length)];
  return s;
}

// 从邮箱库挑一个还没验证的号，填进注册表单。
async function importAccounts(mask) {
  const err = mask.querySelector(".errline");
  const btn = mask.querySelector(".reg-import-btn");
  const sel = mask.querySelector(".reg-acct");
  btn.disabled = true;
  try {
    const r = await invoke("pool_list");
    const list = ((r && r.accounts) || []).filter((a) => a.status !== "verified");
    if (!list.length) {
      err.textContent = t("mb.nothingToVerify");
      return;
    }
    err.textContent = "";
    sel.innerHTML = list
      .map((a) => `<option value="${esc(a.email)}">${esc(a.email)}</option>`)
      .join("");
    sel.hidden = false;
    sel.onchange = () => fillFromPool(mask, sel.value);
    fillFromPool(mask, list[0].email);
  } catch (e) {
    err.textContent = stripErr(e);
  } finally {
    btn.disabled = false;
  }
}

async function fillFromPool(mask, email) {
  try {
    const r = await invoke("pool_get", { email });
    const em = mask.querySelector("#reg-email");
    const pw = mask.querySelector("#reg-pw");
    if (em) em.value = r.email || "";
    if (pw) pw.value = r.password || "";
  } catch (e) {
    const err = mask.querySelector(".errline");
    if (err) err.textContent = stripErr(e);
  }
}

function openAsk(kind) {
  closeModal();
  modalKind = kind;
  const isSignup = kind === "signup";
  const mask = document.createElement("div");
  mask.className = "ov reg bottom";
  mask.innerHTML = `
    <div class="sheet" role="dialog" aria-modal="true">
      <div class="grabber"></div>
      <div class="mhead">${ic(isSignup ? "userPlus" : "mail", 16)} <span>${esc(tr("reg.ask", `${kind}.label`))}</span></div>
      <div class="mbody">
        <div class="mnote" style="margin-bottom:14px">${esc(tr("reg.ask", `${kind}.hint`))}</div>
        ${isSignup
          ? `<div class="importrow">
               <button type="button" class="btn sm reg-import-btn">${ic("import", 13)} ${esc(t("reg.ask.signup.import"))}</button>
               <select class="inp reg-acct" hidden></select>
             </div>`
            + field("reg-name", t("reg.ask.signup.name"), "text", t("reg.ask.signup.namePh"))
            + field("reg-email", t("reg.ask.signup.email"), "email", t("reg.ask.signup.emailPh"))
            + field("reg-pw", t("reg.ask.signup.password"), "password", t("reg.ask.signup.passwordPh"))
          : `<label class="fld">
               <span>${esc(t("reg.ask.link.ph"))}</span>
               <textarea id="reg-link" class="inp" rows="3"
                 placeholder="${esc(t("reg.ask.link.ph"))}" spellcheck="false"></textarea>
             </label>`}
        <div class="errline"></div>
      </div>
      <div class="mfoot">
        <button class="btn g reg-cancel">${esc(t("common.cancel"))}</button>
        <button class="btn p reg-go">${ic("check", 14)} ${esc(t("reg.ask.submit"))}</button>
      </div>
    </div>`;
  document.body.appendChild(mask);
  modal = mask;

  if (isSignup) {
    // 昵称不用手打：给个随机 6 位.
    const nameEl = mask.querySelector("#reg-name");
    if (nameEl && !nameEl.value) nameEl.value = randName();
    const importBtn = mask.querySelector(".reg-import-btn");
    if (importBtn) importBtn.addEventListener("click", () => importAccounts(mask));
  }

  const err = mask.querySelector(".errline");
  const close = () => { closeModal(); };
  mask.addEventListener("click", (e) => { if (e.target === mask) close(); });
  mask.querySelector(".reg-cancel").addEventListener("click", close);

  const first = mask.querySelector("input, textarea");
  if (first) first.focus();

  const submit = async () => {
    let value;
    if (isSignup) {
      const name = mask.querySelector("#reg-name").value.trim();
      const email = mask.querySelector("#reg-email").value.trim();
      const password = mask.querySelector("#reg-pw").value;
      if (!/^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/.test(email)) return (err.textContent = t("reg.err.email"));
      if (password.length < 6) return (err.textContent = t("reg.err.password"));
      // 记下来：注册提交后要拿它去邮箱工具里查激活链接
      S.signupEmail = email;
      value = { name, email, password };
    } else {
      const link = mask.querySelector("#reg-link").value.trim();
      if (!/^https?:\/\//i.test(link)) return (err.textContent = t("reg.err.link"));
      // 人一旦手动粘贴，就不再自动去邮箱取了，避免两条路互相顶
      S.linkAuto = LINK_MAX_AUTO;
      value = link;
    }
    const go = mask.querySelector(".reg-go");
    go.disabled = true;
    try {
      await invoke("reg_input", { ask: kind, value });
      closeModal();
      pushLog("info", isSignup ? "submittedSignup" : "submittedLink");
      render();
      if (drawer) drawer.scrollIntoView({ block: "end" });
    } catch (e) {
      err.textContent = String(e).replace(/^[a-z_]+:/, "");
      go.disabled = false;
    }
  };
  mask.querySelector(".reg-go").addEventListener("click", submit);
  mask.addEventListener("keydown", (e) => {
    if (e.key === "Escape") close();
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) submit();
    else if (e.key === "Enter" && !isSignup) submit();
  });
}

// 事件入口：Rust 侧 app.emit("reg://event", {...})
listen("reg://event", (ev) => regEvent(ev.payload)).catch(() => {});
