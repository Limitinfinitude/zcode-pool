import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n.js";
import { ic } from "./icons.js";
import { esc, toast, openLineModal } from "./ui.js";
import { regStart, regBatchSet } from "./reg.js";

let rerender = () => {};
export function setMboxRerender(fn) {
  rerender = fn;
}

const tr = (group, key) => t(`${group}.${key}`);

const M = {
  list: [],
  filter: "all",
  sel: new Set(),
  batch: { running: false, queue: [], total: 0, done: 0, current: "", results: [] },
  lastRun: null,
};

export function mboxRunning() {
  return M.batch.running;
}

export function mboxStats() {
  const unverified = M.list.filter((a) => a.status !== "verified").length;
  return { total: M.list.length, unverified };
}

export async function mboxLoad() {
  try {
    const r = await invoke("pool_list");
    M.list = (r && r.accounts) || [];
  } catch {
    M.list = [];
  }
  const live = new Set(M.list.map((a) => a.email));
  for (const e of [...M.sel]) if (!live.has(e)) M.sel.delete(e);
}

function filtered() {
  if (M.filter === "new") return M.list.filter((a) => a.status !== "verified");
  if (M.filter === "verified") return M.list.filter((a) => a.status === "verified");
  return M.list;
}

function badge(a) {
  const map = { verified: ["ok", "verified"], failed: ["err", "failed"], invalid: ["err", "invalid"] };
  const [k, key] = map[a.status] || ["new", "new"];
  const note = a.note ? ` title="${esc(a.note)}"` : "";
  return `<span class="chip ${k}"${note}>${esc(tr("mb.status", key))}</span>`;
}

export function mboxPage() {
  const b = M.batch;
  const verified = M.list.filter((a) => a.status === "verified").length;
  const attention = M.list.filter((a) => a.status === "failed" || a.status === "invalid").length;
  const progress = b.running
    ? `<div class="prog">
         <div class="prog-bar"><i style="width:${b.total ? Math.round((b.done / b.total) * 100) : 0}%"></i></div>
         <div class="prog-txt">${esc(t("mb.batch.progress", { done: b.done, total: b.total }))}${b.current ? ` · ${esc(b.current)}` : ""}</div>
       </div>`
    : "";
  const rows = filtered();
  const listHtml = rows.length
    ? rows
        .map(
          (a) => `
      <div class="row" data-email="${esc(a.email)}">
        <input type="checkbox" class="cbx mb-check" aria-label="${esc(a.email)}" data-email="${esc(a.email)}" ${M.sel.has(a.email) ? "checked" : ""}
          ${b.running ? "disabled" : ""}>
        <span class="mail" title="${esc(a.email)}">${esc(a.email)}</span>
        ${badge(a)}
        <span class="when">${esc(a.verified_at || "")}</span>
        ${a.status === "invalid" ? `<button class="btn sm mb-reauth" data-reauth="${esc(a.email)}">${esc(t("mb.reauth"))}</button>` : ""}
        <button class="iconbtn mb-edit" data-edit="${esc(a.email)}" title="${esc(t("mb.update"))}" ${b.running ? "disabled" : ""}>${ic("pen", 14)}</button>
        <button class="iconbtn mb-del" data-del="${esc(a.email)}" title="${esc(t("mb.remove"))}" ${b.running ? "disabled" : ""}>${ic("x", 14)}</button>
      </div>`
        )
        .join("")
    : `<div class="empty">
         <div class="glyph">${ic("empty", 34)}</div>
         <div>${esc(t("mb.empty"))}</div>
         <div class="fmt">
           <div class="fmt-hint">${esc(t("mb.formatHint"))}</div>
           <code>email----password----client_id----refresh_token</code>
         </div>
       </div>`;

  return `
    <div class="content-h">
      <h1>${t("mb.title")}</h1>
      <span class="sub">${t("m.count", { count: M.list.length })}</span>
      <span class="spacer"></span>
      <div class="seg mb-filter" role="group" aria-label="${t("mb.title")}">
        ${["all", "new", "verified"]
          .map((f) => `<button class="${M.filter === f ? "on" : ""}" aria-pressed="${M.filter === f}" click="actions.mboxFilter('${f}')">${tr("mb.filter", f)}</button>`)
          .join("")}
      </div>
    </div>
    <dl class="mail-overview">
      <div><dt>${t("mb.overview.total")}</dt><dd>${M.list.length}</dd></div>
      <div><dt>${t("mb.status.new")}</dt><dd>${M.list.length - verified - attention}</dd></div>
      <div class="verified"><dt>${t("mb.status.verified")}</dt><dd>${verified}</dd></div>
      <div class="attention"><dt>${t("mb.overview.attention")}</dt><dd>${attention}</dd></div>
    </dl>
    <section class="toolbar">
      <button class="btn p" click="actions.mboxImport()" ${b.running ? "disabled" : ""}>${ic("import", 15)} ${t("mb.import")}</button>
      <button class="btn" click="actions.mboxExport()" ${M.list.length && !b.running ? "" : "disabled"}>${ic("export", 14)} ${t("mb.export")}</button>
      <button class="btn" click="actions.mboxVerify()" ${!M.sel.size || b.running ? "disabled" : ""}>${ic("play", 14)} ${t("mb.verify")}${M.sel.size ? ` (${M.sel.size})` : ""}</button>
      <button class="btn" click="actions.mboxStop()" ${b.running ? "" : "disabled"}>${ic("power", 14)} ${t("mb.stop")}</button>
      <span class="spacer"></span>
      <button class="btn d" click="actions.mboxDelete()" ${M.sel.size && !b.running ? "" : "disabled"}>${ic("trash", 14)} ${t("mb.delete")}${M.sel.size ? ` (${M.sel.size})` : ""}</button>
    </section>
    ${!b.running && M.lastRun
      ? (() => {
          const rs = M.lastRun.results || [];
          const ok = rs.filter((r) => r.ok).length;
          const fails = rs.filter((r) => !r.ok);
          return `<div class="mb-result">
            <span class="mb-result-txt">${esc(t("mb.batch.done", { ok, total: M.lastRun.total }))}</span>
            <span class="spacer"></span>
            ${fails.length ? `<button class="btn sm" click="actions.mboxRetryFailed()">${ic("refresh", 13)} ${esc(t("mb.retryFailed", { n: fails.length }))}</button>` : ""}
            <button class="iconbtn" click="actions.mboxDismissResult()" title="${esc(t("mb.dismiss"))}" style="color:var(--label-2)">${ic("x", 14)}</button>
          </div>
          ${fails.length ? `<div class="mb-fails">${fails.map((r) => `<div class="mb-fail"><span class="mb-fail-mail">${esc(r.email)}</span><span class="mb-fail-why">${esc(r.error || t("m.unknownErr"))}</span></div>`).join("")}</div>` : ""}`;
        })()
      : ""}
    ${M.list.length && !b.running ? `
      <div class="mb-fmt">
        <span>${esc(t("mb.formatLabel"))}</span>
        <code>email----password----client_id----refresh_token</code>
      </div>
      <div class="mb-tip">${ic("alert", 13)}<span>${esc(t("mb.tip"))}</span></div>` : ""}
    ${progress}
    <div class="list">
      ${rows.length
        ? `<div class="list-head">
             <label class="selall">
               <input type="checkbox" class="cbx" id="mb-all" ${selAllOn(rows) ? "checked" : ""} ${b.running ? "disabled" : ""}>
               <span>${t("mb.selectAll")}</span>
             </label>
             <span class="selinfo">${M.sel.size ? t("mb.selected", { n: M.sel.size }) : ""}</span>
             <span class="spacer"></span>
             ${M.sel.size ? `<button class="btn g sm" click="actions.mboxSelectNone()">${t("mb.selectNone")}</button>` : ""}
           </div>`
        : ""}
      <div class="group">${listHtml}</div>
    </div>`;
}

function selAllOn(rows) {
  const newOnes = rows.filter((a) => a.status === "new");
  return newOnes.length > 0 && newOnes.every((a) => M.sel.has(a.email));
}


export function mboxFilter(f) {
  M.filter = f;
  rerender();
}

export function mboxToggle(email, on) {
  if (on) M.sel.add(email);
  else M.sel.delete(email);
}

export function mboxSelectAll(on) {
  if (M.batch.running) return;
  for (const a of filtered()) {
    if (a.status !== "new") continue;
    if (on) M.sel.add(a.email);
    else M.sel.delete(a.email);
  }
  rerender();
}

export function mboxSelectNone() {
  M.sel.clear();
  rerender();
}

export function mboxToggleRow(email) {
  if (M.batch.running || !email) return;
  if (M.sel.has(email)) M.sel.delete(email);
  else M.sel.add(email);
  rerender();
}

export async function mboxImport() {
  try {
    const r = await invoke("pool_import_pick");
    if (!r || !r.picked) return;
    await mboxLoad();
    rerender();
    if (r.added > 0) toast(t("mb.importOk", { added: r.added, skipped: r.skipped }), "ok");
    else if (r.parsed === 0) toast(t("mb.importEmpty"), "err");
    else toast(t("mb.importDup", { skipped: r.skipped }), "warn");
  } catch (e) {
    toast(String(e).replace(/^[a-z_]+:/, ""), "err");
  }
}

export async function mboxRemove(email) {
  try {
    await invoke("pool_remove", { email });
    M.sel.delete(email);
    await mboxLoad();
    rerender();
  } catch (e) {
    toast(String(e).replace(/^[a-z_]+:/, ""), "err");
  }
}

export function mboxSelection() {
  return [...M.sel];
}

export async function mboxDelete() {
  const emails = [...M.sel];
  if (!emails.length) return 0;
  try {
    const r = await invoke("pool_remove_many", { emails });
    M.sel.clear();
    await mboxLoad();
    rerender();
    toast(t("mb.deleted", { n: r.removed }), "ok");
    return r.removed;
  } catch (e) {
    toast(String(e).replace(/^[a-z_]+:/, ""), "err");
    return 0;
  }
}

export async function mboxExport() {
  try {
    const r = await invoke("pool_export");
    if (!r || !r.picked) return;
    toast(t("m.exportSavedAll", { count: r.count }), "ok", r.path);
  } catch (e) {
    toast(String(e).replace(/^[a-z_]+:/, ""), "err");
  }
}

export function mboxUpdateLine(email) {
  openLineModal({
    title: t("mb.updateTitle", { email }),
    hint: t("mb.updateHint"),
    label: t("mb.updateLine"),
    placeholder: "email----password----client_id----refresh_token",
    onSubmit: async (line) => {
      const r = await invoke("pool_upsert", { line });
      await mboxLoad();
      rerender();
      toast(t("mb.updated", { email: r.email }), "ok");
    },
  });
}

export async function mboxReauth(email) {
  let info;
  try {
    info = await invoke("outlook_reauth_begin", { email });
  } catch (e) {
    toast(String(e).replace(/^[a-z_]+:/, ""), "err");
    return;
  }
  const ov = document.createElement("div");
  ov.className = "ov reauth";
  ov.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true">
      <div class="mhead">${ic("lock", 16)} <span>${esc(t("mb.reauthTitle", { email }))}</span></div>
      <div class="mbody">
        <div class="mnote" style="margin-bottom:12px">${esc(t("mb.reauthHint"))}</div>
        <div class="reauth-code">${esc(info.user_code)}</div>
        <div class="line" style="margin-top:10px">
          <button class="btn g ra-open">${ic("export", 14)} ${esc(t("mb.reauthOpen"))}</button>
          <button class="btn g ra-copy">${esc(t("mb.reauthCopy"))}</button>
        </div>
        <div class="errline ra-err"></div>
      </div>
      <div class="mfoot"><button class="btn g ra-close">${t("common.cancel")}</button></div>
    </div>`;
  document.body.appendChild(ov);
  const errEl = ov.querySelector(".ra-err");
  let stopped = false;
  const close = () => {
    stopped = true;
    ov.remove();
  };
  ov.querySelector(".ra-open").addEventListener("click", () => invoke("open_external", { url: info.verification_uri }).catch(() => {}));
  ov.querySelector(".ra-copy").addEventListener("click", () => { navigator.clipboard?.writeText(info.user_code).catch(() => {}); });
  ov.querySelector(".ra-close").addEventListener("click", close);
  ov.addEventListener("click", (e) => { if (e.target === ov) close(); });

  const gap = Math.max(3, info.interval || 5) * 1000;
  const max = Math.ceil((info.expires_in || 900) / Math.max(3, info.interval || 5));
  let tries = 0;
  const tick = async () => {
    if (stopped) return;
    tries += 1;
    if (tries > max) {
      errEl.textContent = t("mb.reauthExpired");
      return;
    }
    try {
      const r = await invoke("outlook_reauth_poll", { email, clientId: info.client_id, deviceCode: info.device_code });
      if (!r.pending) {
        toast(t("mb.reauthOk", { email }), "ok");
        close();
        await mboxLoad();
        rerender();
        return;
      }
    } catch (e) {
      const msg = String(e).replace(/^[a-z_]+:/, "");
      if (/expired|declined|denied|bad_verification|not.?found|invalid/i.test(msg)) {
        errEl.textContent = msg;
        return;
      }
    }
    setTimeout(tick, gap);
  };
  setTimeout(tick, 1500);
}


export async function mboxVerify() {
  const emails = [...M.sel].filter((e) => {
    const a = M.list.find((x) => x.email === e);
    return a && a.status === "new";
  });
  if (!emails.length) {
    toast(t("mb.nothingToVerify"), "warn");
    return;
  }
  M.batch = { running: true, queue: emails.slice(), total: emails.length, done: 0, current: "", results: [] };
  rerender();
  runNext();
}

async function runNext() {
  const b = M.batch;
  if (!b.running) return;
  if (!b.queue.length) return finishBatch();
  const email = b.queue.shift();
  b.current = email;
  rerender();
  try {
    regStart("register", "zai");
    regBatchSet(email);
    await invoke("oauth_begin", { provider: "zai", mode: "register", batch: true });
  } catch (e) {
    const msg = String(e).replace(/^[a-z_]+:/, "");
    b.done += 1;
    b.results.push({ email, ok: false, error: msg });
    await invoke("pool_mark_verified", { email, ok: false, note: msg }).catch(() => {});
    setTimeout(runNext, 400);
  }
}

export function mboxOnOauthDone(email, ok, note) {
  const b = M.batch;
  if (!b.running || !email) return false;
  b.done += 1;
  b.results.push({ email, ok, error: note || "" });
  invoke("pool_mark_verified", { email, ok, note: note || null }).catch(() => {});
  mboxLoad().then(rerender);
  setTimeout(runNext, 900);
  return true;
}

export function mboxStop() {
  const b = M.batch;
  if (!b.running) return;
  b.running = false;
  b.current = "";
  regBatchSet("");
  invoke("reg_close_window").catch(() => {});
  toast(t("mb.batch.stopped"), "warn");
  rerender();
}

export function mboxSkipCurrent() {
  const b = M.batch;
  if (!b.running || !b.current) {
    toast(t("mb.batch.noCurrent"), "warn");
    return;
  }
  const email = b.current;
  b.current = "";
  b.done += 1;
  const note = t("mb.batch.skipped");
  b.results.push({ email, ok: false, error: note });
  invoke("pool_mark_verified", { email, ok: false, note }).catch(() => {});
  regBatchSet("");
  invoke("reg_close_window").catch(() => {});
  toast(t("mb.batch.skippedToast", { email }), "warn");
  mboxLoad().then(rerender);
  setTimeout(runNext, 600);
}

function finishBatch() {
  const b = M.batch;
  const ok = b.results.filter((r) => r.ok).length;
  b.running = false;
  b.current = "";
  regBatchSet("");
  M.lastRun = { total: b.total, results: b.results.slice() };
  toast(t("mb.batch.done", { ok, total: b.total }), ok === b.total ? "ok" : "warn");
  rerender();
}

export async function mboxRetryFailed() {
  const fails = (M.lastRun?.results || []).filter((r) => !r.ok).map((r) => r.email).filter(Boolean);
  if (!fails.length) return;
  for (const email of fails) {
    await invoke("pool_reset", { email }).catch(() => {});
  }
  await mboxLoad();
  M.sel = new Set(fails);
  M.lastRun = null;
  rerender();
  await mboxVerify();
}

export function mboxDismissResult() {
  M.lastRun = null;
  rerender();
}

export function mboxCurrent() {
  return M.batch.running ? M.batch.current : "";
}
