import { invoke } from "@tauri-apps/api/core";
import { ic } from "./icons.js";
import { t, has, errCode, stripErr } from "./i18n.js";

document.addEventListener("contextmenu", (e) => e.preventDefault());

document.addEventListener("keydown", (e) => {
  if (e.key === "F12") e.preventDefault();
  if (e.ctrlKey && e.shiftKey && ["I", "i", "J", "j", "C", "c"].includes(e.key)) e.preventDefault();
  if (e.ctrlKey && ["u", "s"].includes(e.key.toLowerCase())) e.preventDefault();
});

export function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

export function toast(msg, kind = "ok", detail = "") {
  let zone = document.querySelector(".toasts");
  if (!zone) {
    zone = document.createElement("div");
    zone.className = "toasts";
    document.body.appendChild(zone);
  }
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.innerHTML = `<span class="ti">${ic(kind === "ok" ? "check" : "alert", 16)}</span>
    <span class="tb">${esc(msg)}${detail ? `<span class="d">${esc(detail)}</span>` : ""}</span>`;
  el.addEventListener("click", () => el.remove());
  zone.appendChild(el);
  setTimeout(() => el.remove(), detail ? 5200 : 3200);
}

// [click="expr"] / [keydown="expr"] 事件代理
function runAttr(expr, event) {
  try {
    const open = expr.indexOf("(");
    if (open < 0 || !expr.trimEnd().endsWith(")")) return;
    let fn = window;
    for (const seg of expr.slice(0, open).trim().split(".")) fn = fn?.[seg];
    if (typeof fn !== "function") return;
    const src = expr.slice(open + 1, expr.trimEnd().length - 1).trim();
    const args = src
      ? src.split(/\s*,\s*/).map((a) => {
          if (a === "event") return event;
          const m = a.match(/^'([^']*)'$/);
          if (m) return m[1];
          return JSON.parse(a);
        })
      : [];
    fn(...args);
  } catch (e) {
    console.warn("attr handler error:", expr, e);
  }
}

export function installDelegation() {
  document.addEventListener("click", (e) => {
    const el = e.target.closest("[click]");
    if (!el) return;
    e.preventDefault();
    runAttr(el.getAttribute("click") || "", e);
  });
  document.addEventListener("keydown", (e) => {
    const el = e.target.closest("[keydown]");
    if (!el) return;
    runAttr(el.getAttribute("keydown") || "", e);
  });
  document.addEventListener("blur", (e) => {
    const el = e.target.closest("[blur]");
    if (!el) return;
    runAttr(el.getAttribute("blur") || "", e);
  }, true);
}

/** 通用确认框。kind: plain / danger。 */
export function openConfirmModal(m) {
  document.querySelector(".ov.confirm")?.remove();
  const kind = m.kind || "plain";
  const ov = document.createElement("div");
  ov.className = "ov confirm alert";
  ov.innerHTML = `
    <div class="modal" role="alertdialog" aria-modal="true" aria-label="${esc(m.title)}">
      <div class="mhead">
        <span style="color:${kind === "danger" ? "var(--err)" : kind === "warn" ? "var(--warn)" : "var(--acc)"}">${ic(m.icon || "alert", 18)}</span>
        <span>${esc(m.title)}</span>
      </div>
      ${m.desc ? `<div class="mbody"><div class="mnote">${m.desc}</div></div>` : ""}
      <div class="mfoot">
        <button class="btn g cf-no">${esc(m.noLabel || t("common.cancel"))}</button>
        <button class="btn ${kind === "danger" ? "d" : "p"} cf-yes">${esc(m.yesLabel || t("common.confirm"))}</button>
      </div>
    </div>`;
  document.body.appendChild(ov);

  const yes = ov.querySelector(".cf-yes");
  const no = ov.querySelector(".cf-no");
  const close = () => { document.removeEventListener("keydown", ov._key); ov.remove(); };
  ov.querySelector(".modal").addEventListener("click", (e) => e.stopPropagation());
  ov.addEventListener("click", close);
  no.addEventListener("click", close);
  yes.addEventListener("click", async () => {
    yes.disabled = true; no.disabled = true;
    try { await m.onYes?.(); } finally { close(); }
  });
  ov._key = (e) => {
    if (e.key === "Escape") close();
    if (e.key === "Enter" && e.target === document.body && !yes.disabled) yes.click();
  };
  document.addEventListener("keydown", ov._key);
  (m.focusNo || kind === "danger" ? no : yes).focus();
}

/** 粘一行文本的小弹窗（用于「更新凭据」）。onSubmit(value) 返回 Promise。 */
export function openLineModal(m) {
  document.querySelector(".ov.line")?.remove();
  const ov = document.createElement("div");
  ov.className = "ov line";
  ov.innerHTML = `
    <div class="modal wide" role="dialog" aria-modal="true">
      <div class="mhead">${ic("pen", 16)} <span>${esc(m.title)}</span>
        <span class="spacer"></span>
        <button class="iconbtn lm-x" style="color:var(--label-2)">${ic("x", 15)}</button>
      </div>
      <div class="mbody">
        <div class="mnote" style="margin-bottom:12px">${esc(m.hint || "")}</div>
        <label class="fld"><span>${esc(m.label || "")}</span>
          <textarea class="inp" rows="3" placeholder="${esc(m.placeholder || "")}" spellcheck="false"></textarea></label>
        <div class="errline"></div>
      </div>
      <div class="mfoot">
        <button class="btn g lm-cancel">${t("common.cancel")}</button>
        <button class="btn p lm-go">${ic("check", 14)} ${t("common.save")}</button>
      </div>
    </div>`;
  document.body.appendChild(ov);

  const ta = ov.querySelector("textarea");
  const err = ov.querySelector(".errline");
  const go = ov.querySelector(".lm-go");
  const close = () => ov.remove();
  ov.querySelector(".lm-x").addEventListener("click", close);
  ov.querySelector(".lm-cancel").addEventListener("click", close);
  ov.addEventListener("click", (e) => { if (e.target === ov) close(); });
  ov.querySelector(".modal").addEventListener("click", (e) => e.stopPropagation());
  const submit = async () => {
    const v = ta.value.trim();
    if (!v) return (err.textContent = t("mb.updateHint"));
    go.disabled = true;
    try { await m.onSubmit?.(v); close(); }
    catch (e) { err.textContent = stripErr(e); go.disabled = false; }
  };
  go.addEventListener("click", submit);
  ta.addEventListener("keydown", (e) => { if (e.key === "Escape") close(); if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) submit(); });
  setTimeout(() => ta.focus(), 30);
}

/**
 * 添加账号：选一个登录站点，在登录窗里**手动**登录。
 * 不做任何自动化 —— 自动注册 / 验证在「邮箱库」那边的批量自动验证里。
 */
export function openAddAccountModal(m) {
  document.querySelector(".ov.add")?.remove();
  const providers = m.providers || [];
  const displayOf = (p) => (has(`prov.${p.id}`) ? t(`prov.${p.id}`) : p.display);

  const ov = document.createElement("div");
  ov.className = "ov add";
  ov.innerHTML = `
    <div class="modal wide" role="dialog" aria-modal="true">
      <div class="mhead">${ic("userPlus", 16)} <span>${t("prov.title")}</span>
        <span class="spacer"></span>
        <button class="iconbtn add-x" aria-label="${t("common.cancel")}" style="color:var(--label-2)">${ic("x", 15)}</button>
      </div>
      <div class="mbody add-body">
        <div class="mnote" style="margin-bottom:12px">${t("prov.sub")}</div>
        ${providers.map((p) => `
          <button class="opt" data-id="${esc(p.id)}">
            <span class="ocol"><span class="oname">${esc(displayOf(p))}</span></span>
            <span class="arrow">→</span>
          </button>`).join("")}
        <div class="mfoot" style="border-top:0;padding:12px 0 0">
          <button class="btn g add-cancel">${t("common.cancel")}</button>
        </div>
      </div>
    </div>`;
  document.body.appendChild(ov);

  const onKey = (e) => { if (e.key === "Escape") close(); };
  const close = () => { ov.remove(); document.removeEventListener("keydown", onKey); };
  document.addEventListener("keydown", onKey);
  ov.addEventListener("click", (e) => { if (e.target === ov) close(); });
  ov.querySelector(".add-x").addEventListener("click", close);
  ov.querySelector(".add-cancel").addEventListener("click", close);

  ov.querySelectorAll(".opt").forEach((b) => {
    b.addEventListener("click", () => {
      close();
      m.onStart?.(b.dataset.id, "observe"); // 一律手动：只开登录窗，不自动化
    });
  });
  ov.querySelector(".opt")?.focus();
}
