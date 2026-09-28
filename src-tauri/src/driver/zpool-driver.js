(function () {
  "use strict";
  if (window.__zpool) return;
  if (window.top !== window.self) return;

  var CFG = window.__ZPOOL_CFG || {};
  var POLL_MS = 900;
  var FORM_SETTLE_MS = 20000;
  var RETRY_BACKOFF_MS = 12000;
  var FINISH_WAIT_MS = 25000;
  var SIGNUP_URL = "https://chat.z.ai/auth?action=signup";
  var STORE_KEY = "zpool-driver-state";
  var SIGNUP_HYDRATE_MS = 4500;
  var SIGNUP_RELOAD_MAX = 2;
  var SIGNUP_ENTRY_RE = /^(注册|免费注册|立即注册|创建账号|Sign up|Create account|Register)$/;
  var SIGNUP_ENTRY_WAIT_MS = 9000; 
  var SIGNUP_CLICK_WAIT_MS = 6000; 

  var S = {
    mode: CFG.mode || "observe",
    auto: !!CFG.auto,
    provider: CFG.provider || "",
    returnUrl: CFG.return_url || "",
    phase: "",
    page: "",
    asked: {},
    answered: {},
    password: "",
    email: "",
    link: "",
    wentSignup: false,
    signupTries: 0,
    submitted: false,
    signupAt: 0,
    linkOpened: false,
    backDone: false,
    lastSubmitAt: 0,
    lastCode: "",
    submitGateAt: 0,
    finishTried: false,
    authorized: false,
    stopped: false,
    lastLogKey: "",
    noteKey: "",
    signupReloads: 0,
    signupClicked: false,
    signupClickAt: 0,
    entrySince: 0,
    capSnap: "",
    gateWarned: false,
    timer: 0
  };

  function saveState() {
    try {
      sessionStorage.setItem(STORE_KEY, JSON.stringify({
        mode: S.mode,
        auto: S.auto,
        phase: S.phase,
        asked: S.asked,
        answered: S.answered,
        password: S.password,
        email: S.email,
        link: S.link,
        wentSignup: S.wentSignup,
        signupTries: S.signupTries,
        submitted: S.submitted,
        signupAt: S.signupAt,
        linkOpened: S.linkOpened,
        backDone: S.backDone,
        finishTried: S.finishTried,
        submitGateAt: S.submitGateAt,
        lastSubmitAt: S.lastSubmitAt,
        lastLogKey: S.lastLogKey,
        noteKey: S.noteKey,
        signupReloads: S.signupReloads,
        signupClicked: S.signupClicked,
        signupClickAt: S.signupClickAt,
        entrySince: S.entrySince
      }));
    } catch (e) {
    }
  }

  (function restore() {
    var saved = null;
    try {
      var raw = sessionStorage.getItem(STORE_KEY);
      saved = raw ? JSON.parse(raw) : null;
    } catch (e) {
      saved = null;
    }
    if (!saved || typeof saved !== "object") return;
    if (typeof saved.mode === "string" && saved.mode) S.mode = saved.mode;
    if (typeof saved.auto === "boolean") S.auto = saved.auto;
    if (typeof saved.phase === "string") S.phase = saved.phase;
    if (saved.asked && typeof saved.asked === "object") S.asked = saved.asked;
    if (saved.answered && typeof saved.answered === "object") S.answered = saved.answered;
    S.password = saved.password || "";
    S.email = saved.email || "";
    S.link = saved.link || "";
    S.wentSignup = !!saved.wentSignup;
    S.signupTries = Number(saved.signupTries) || 0;
    S.submitted = !!saved.submitted;
    S.signupAt = Number(saved.signupAt) || 0;
    S.linkOpened = !!saved.linkOpened;
    S.backDone = !!saved.backDone;
    S.finishTried = !!saved.finishTried;
    S.submitGateAt = Number(saved.submitGateAt) || 0;
    S.lastSubmitAt = Number(saved.lastSubmitAt) || 0;
    S.lastLogKey = saved.lastLogKey || "";
    S.noteKey = saved.noteKey || "";
    S.signupReloads = Number(saved.signupReloads) || 0;
    S.signupClicked = !!saved.signupClicked;
    S.signupClickAt = Number(saved.signupClickAt) || 0;
    S.entrySince = Number(saved.entrySince) || 0;
  })();

  function q(kv) {
    var out = [];
    for (var k in kv) {
      if (!Object.prototype.hasOwnProperty.call(kv, k)) continue;
      var v = kv[k];
      if (v === null || v === undefined || v === "") continue;
      out.push(encodeURIComponent(k) + "=" + encodeURIComponent(String(v)));
    }
    return out.join("&");
  }

  var pageReady = document.readyState !== "loading";
  var outbox = [];
  var flushing = false;
  var flushTimer = 0;
  var retryTimer = 0;
  var halted = false;

  function go(url) {
    try {
      window.location.href = url;
    } catch (e) {}
  }

  function reloadPage() {
    halted = true;
    clearTimeout(flushTimer);
    clearTimeout(retryTimer);
    flushing = false;
    drainNow();
    setTimeout(function () {
      try {
        location.reload();
      } catch (e) {}
      var tries = 0;
      function retry() {
        tries += 1;
        if (tries > 2) {
          halted = false;
          return;
        }
        try {
          location.reload();
        } catch (e) {}
        retryTimer = setTimeout(retry, 4000);
      }
      retryTimer = setTimeout(retry, 4000);
    }, 80);
  }

  function flush() {
    if (halted || flushing || !outbox.length) return;
    flushing = true;
    var msg = outbox.shift();
    go("zpool-driver://msg?" + q(msg));
    flushTimer = setTimeout(function () {
      flushing = false;
      flush();
    }, 160);
  }

  function send(msg) {
    if (halted) return;
    outbox.push(msg);
    if (outbox.length > 80) outbox.splice(0, outbox.length - 80);
    if (pageReady) flush();
  }

  function drainNow() {
    while (outbox.length) {
      var m = outbox.shift();
      go("zpool-driver://msg?" + q(m));
    }
  }

  function navTo(url) {
    halted = true;
    clearTimeout(flushTimer);
    clearTimeout(retryTimer);
    flushing = false;
    drainNow();
    setTimeout(function () {
      go(url);
      var tries = 0;
      function retry() {
        tries += 1;
        if (tries > 2) {
          halted = false; 
          fail("navStuck", url);
          return;
        }
        go(url);
        retryTimer = setTimeout(retry, 4000);
      }
      retryTimer = setTimeout(retry, 4000);
    }, 80);
  }

  function markReady() {
    if (pageReady) return;
    pageReady = true;
    flush();
  }

  if (pageReady) {
    markReady();
  } else {
    document.addEventListener("DOMContentLoaded", function () {
      setTimeout(markReady, 250);
    }, { once: true });
    setTimeout(markReady, 4000);
  }

  function emit(level, code, a, b) {
    var key = level + "|" + code + "|" + (a || "") + "|" + (b || "");
    if (S.lastLogKey === key) return;
    S.lastLogKey = key;
    send({ kind: "log", level: level, code: code, a: a || "", b: b || "" });
  }

  function log(code, a, b) { emit("info", code, a, b); }
  function ok(code, a, b) { emit("ok", code, a, b); }
  function warn(code, a, b) { emit("warn", code, a, b); }
  function fail(code, a, b) { emit("error", code, a, b); }

  function phase(p) {
    if (S.phase === p) return;
    S.phase = p;
    S.lastLogKey = "";
    saveState();
    send({ kind: "phase", phase: p });
  }

  function note(code, a) {
    var key = code + "|" + (a || "");
    if (S.noteKey === key) return;
    S.noteKey = key;
    saveState();
    send({ kind: "note", code: code, a: a || "" });
  }

  function clearNote() {
    note("clear");
  }

  function ask(kind, force) {
    if (S.asked[kind] && !force) return false;
    S.asked[kind] = true;
    send({ kind: "ask", ask: kind });
    return true;
  }

  function once(key) {
    if (S.asked[key]) return false;
    S.asked[key] = true;
    return true;
  }

  function sleep(ms) {
    return new Promise(function (r) { setTimeout(r, ms); });
  }

  function vis(e) {
    return !!e && e.offsetParent !== null;
  }

  function allInputs() {
    return Array.prototype.slice.call(document.querySelectorAll("input, textarea")).filter(vis);
  }

  function btns() {
    return Array.prototype.slice
      .call(document.querySelectorAll("button, a, div, span, [role=button]"))
      .filter(vis);
  }

  function normText(s) {
    return String(s || "").replace(/\s+/g, "");
  }

  function bodyText() {
    return String((document.body && document.body.innerText) || "").replace(/\s+/g, " ");
  }

  function fill(el, value) {
    if (!el) return false;
    try {
      var desc = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value");
      if (desc && desc.set) desc.set.call(el, String(value));
      else el.value = String(value);
    } catch (e) {
      try { el.value = String(value); } catch (e2) { return false; }
    }
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    el.dispatchEvent(new Event("blur", { bubbles: true }));
    return String(el.value || "") === String(value);
  }

  function clickText(list) {
    var want = list.map(function (t) { return normText(t); });
    var cands = btns().filter(function (e) {
      return want.indexOf(normText(e.innerText)) >= 0;
    });
    if (!cands.length) return false;
    var btn = null;
    for (var i = 0; i < cands.length; i++) {
      if (cands[i].tagName === "BUTTON") { btn = cands[i]; break; }
    }
    if (!btn) btn = cands[cands.length - 1];
    try { btn.click(); } catch (e) { return false; }
    return true;
  }

  function clickEnabled(list) {
    var want = list.map(function (t) { return normText(t); });
    var cands = btns().filter(function (e) {
      return want.indexOf(normText(e.innerText)) >= 0;
    });
    if (!cands.length) return false;
    var btn = null;
    for (var i = 0; i < cands.length; i++) {
      if (cands[i].tagName === "BUTTON") { btn = cands[i]; break; }
    }
    if (!btn) btn = cands[cands.length - 1];
    if (btn.disabled || btn.getAttribute("aria-disabled") === "true") return false;
    try { btn.click(); } catch (e) { return false; }
    return true;
  }

  function findButton(res) {
    return btns().filter(function (e) {
      return e.tagName === "BUTTON" && res.test(normText(e.innerText));
    })[0] || null;
  }

  function currentPage() {
    var host = location.hostname;
    var path = location.pathname;
    var action = "";
    try { action = new URLSearchParams(location.search).get("action") || ""; } catch (e) { action = ""; }
    if (host.indexOf("chat.z.ai") >= 0) {
      if (path.indexOf("/auth") === 0) return action === "signup" ? "zai-signup" : "zai-auth";
      if (path.indexOf("/api/oauth/authorize") === 0) return "zai-authorize";
      return "zai-other";
    }
    if (host.indexOf("bigmodel.cn") >= 0) return "bigmodel-login";
    if (host.indexOf("zcode.z.ai") >= 0) return "zcode-bridge";
    return "other";
  }

  function emailInput() {
    var ins = allInputs();
    return ins.filter(function (e) { return e.type === "email"; })[0]
      || ins.filter(function (e) { return /邮箱|mail/i.test(e.placeholder || ""); })[0];
  }

  function passwordInput() {
    var ins = allInputs();
    return ins.filter(function (e) { return e.type === "password"; })[0]
      || ins.filter(function (e) { return /密码|password/i.test(e.placeholder || ""); })[0];
  }

  function nameInput() {
    var ins = allInputs();
    return ins.filter(function (e) { return /名称|昵称|名字|name/i.test(e.placeholder || ""); })[0]
      || ins.filter(function (e) {
        return e.type === "text" && !/邮箱|mail|手机|phone|验证码|code/i.test(e.placeholder || "");
      })[0];
  }

  function signupNameInput() {
    return allInputs().filter(function (e) {
      return /名称|昵称|名字|name/i.test(e.placeholder || "");
    })[0];
  }

  function probeSignup() {
    var e = emailInput();
    var p = passwordInput();
    var n = nameInput();
    var body = bodyText();
    var verifyBtn = btns().filter(function (x) {
      return /^(点击开始验证|Click to start verification)$/.test(normText(x.innerText));
    })[0];
    return {
      hasForm: !!e && !!p,
      name: n ? String(n.value || "").trim() : "",
      email: e ? String(e.value || "").trim() : "",
      password: p ? String(p.value || "") : "",
      nameOk: n ? String(n.value || "").trim().length > 0 : true,
      emailOk: /^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/.test(e ? String(e.value || "").trim() : ""),
      pwdOk: p ? String(p.value || "").length >= 6 : false,
      hasCreate: !!findButton(/^(创建账号|注册|Sign up|Create account)$/),
      hasNameField: !!signupNameInput(),
      hasVerifyBtn: !!verifyBtn,
      verifyPassed: !verifyBtn && /验证通过|验证成功|Verified/i.test(body),
      sentHint: /验证邮件|已发送|查收|check your email|verify your email/i.test(body),
      errorHint: (body.match(/(错误|失败|频繁|重试|稍后|无效|已被|已存在|重复|invalid|error|too many|rate limit|try again)[^\s。]{0,40}/i) || [])[0] || null
    };
  }

  function probeFinish() {
    var pws = allInputs().filter(function (e) { return e.type === "password"; });
    return {
      pwdCount: pws.length,
      hasButton: !!findButton(/^(完成注册|完成|设置密码|Finish|Continue|继续)$/)
    };
  }

  function probePhone() {
    var ins = allInputs();
    var digits = function (v) { return String(v || "").replace(/\D/g, ""); };
    var phoneEl = ins.filter(function (e) { return /手机|电话|phone/i.test(e.placeholder || ""); })[0];
    var codeEl = ins.filter(function (e) { return /验证码|校验码|code/i.test(e.placeholder || ""); })[0];
    var btn = findButton(/^登录\s*\/\s*注\s*册$/);
    var needAgree = Array.prototype.slice.call(document.querySelectorAll("input[type=checkbox]"))
      .filter(vis).some(function (c) { return !c.checked; });
    return {
      hasForm: !!phoneEl && !!codeEl,
      phone: phoneEl ? digits(phoneEl.value) : "",
      code: codeEl ? digits(codeEl.value) : "",
      phoneOk: phoneEl ? digits(phoneEl.value).length >= 11 : false,
      codeOk: codeEl ? digits(codeEl.value).length >= 4 : false,
      needAgree: needAgree,
      btnOk: !!btn && !btn.disabled && btn.getAttribute("aria-disabled") !== "true"
    };
  }

  function onAuthorizePage() {
    var body = bodyText();
    if (/请输入验证码|请输入手机号|输入您的电子邮箱|输入您的密码|创建账号|Enter your email|Create account/i.test(body)) {
      return false;
    }
    return btns().some(function (b) {
      return /^(继续|同意|确认授权|授权|确认|Continue|Authorize|Allow)$/.test(normText(b.innerText));
    });
  }

  function agreementBoxes() {
    var sel = 'input[type="checkbox"], .el-checkbox__original, .ant-checkbox-input, [role="checkbox"]';
    return Array.prototype.slice.call(document.querySelectorAll(sel)).filter(vis);
  }

  function deepText() {
    var parts = [];
    var queue = [document];
    var guard = 0;
    while (queue.length && guard++ < 40) {
      var root = queue.shift();
      try {
        if (root.body) {
          if (root.body.innerText) parts.push(root.body.innerText);
        } else if (root.innerText) {
          parts.push(root.innerText);
        }
      } catch (e) {}
      var els;
      try {
        els = root.querySelectorAll ? root.querySelectorAll("*") : [];
      } catch (e) {
        continue;
      }
      for (var i = 0; i < els.length && i < 4000; i++) {
        var el = els[i];
        try {
          if (el.shadowRoot) queue.push(el.shadowRoot);
        } catch (e) {}
        if (el.tagName === "IFRAME") {
          try {
            var d = el.contentDocument;
            if (d && d.body) queue.push(d);
          } catch (e) {}
        }
      }
    }
    return parts.join(" ").replace(/\s+/g, " ");
  }

  function deepFields() {
    var out = [];
    var roots = [document];
    var guard = 0;
    while (roots.length && guard++ < 40) {
      var root = roots.shift();
      try {
        var fs = root.querySelectorAll
          ? root.querySelectorAll('input[type="hidden"], textarea, input[name]')
          : [];
        for (var i = 0; i < fs.length; i++) out.push(fs[i]);
      } catch (e) {}
      var els;
      try {
        els = root.querySelectorAll ? root.querySelectorAll("*") : [];
      } catch (e) {
        continue;
      }
      for (var j = 0; j < els.length && j < 4000; j++) {
        try {
          if (els[j].shadowRoot) roots.push(els[j].shadowRoot);
        } catch (e) {}
      }
    }
    return out;
  }

  var CAPTCHA_OK_RE = /验证通过|验证成功|验证完成|已通过|Verified|Success/i;
  var CAPTCHA_FIELD_RE = /captcha|verify|aliyun|nvc|token|nc_|slide/i;

  function captchaVerified() {
    try {
      if (CAPTCHA_OK_RE.test(deepText())) return true;
      var fields = deepFields();
      for (var i = 0; i < fields.length; i++) {
        var f = fields[i];
        var name = (f.name || "") + (f.id || "") + (f.className || "");
        if (CAPTCHA_FIELD_RE.test(name) && String(f.value || "").length >= 12) {
          return true;
        }
      }
    } catch (e) {}
    return false;
  }

  function createBtnState() {
    var b = findButton(/^(创建账号|注册|Sign up|Create account)$/);
    if (!b) return { found: false, disabled: false };
    var disabled =
      b.disabled === true ||
      b.getAttribute("aria-disabled") === "true" ||
      /disabled/i.test(String(b.className || ""));
    return { found: true, disabled: disabled };
  }

  function captchaSnapshot() {
    var n = function (sel) {
      try {
        return document.querySelectorAll(sel).length;
      } catch (e) {
        return -1;
      }
    };
    var sh = 0;
    var ifr = 0;
    try {
      var els = document.querySelectorAll("*");
      for (var i = 0; i < els.length && i < 5000; i++) {
        if (els[i].shadowRoot) sh++;
        if (els[i].tagName === "IFRAME") ifr++;
      }
    } catch (e) {}
    var fs = deepFields();
    var fC = 0; 
    for (var j = 0; j < fs.length; j++) {
      var nm = (fs[j].name || "") + (fs[j].id || "") + (fs[j].className || "");
      if (CAPTCHA_FIELD_RE.test(nm) && String(fs[j].value || "").length >= 8) fC++;
    }
    var btn = createBtnState();
    return (
      "ok=" + (CAPTCHA_OK_RE.test(deepText()) ? 1 : 0) +
      " nc=" + n("[class*=nc_],[id*=nc_],[class*=aliyun],[id*=aliyun]") +
      " sh=" + sh + " ifr=" + ifr + " fC=" + fC +
      " b=" + (btn.found ? (btn.disabled ? "dis" : "on") : "none")
    );
  }

  function checkAgreement() {
    var boxes = agreementBoxes();
    if (boxes.length) {
      boxes.forEach(function (cb) {
        var on = cb.checked === true || cb.getAttribute("aria-checked") === "true";
        if (!on) { try { cb.click(); } catch (e) {} }
      });
      return true;
    }
    var hit = Array.prototype.slice
      .call(document.querySelectorAll("label, span, div, a"))
      .filter(vis)
      .filter(function (e) {
        return /用户协议|隐私政策|同意.*协议|agree|terms|privacy/i.test(normText(e.innerText));
      })
      .sort(function (a, b) {
        return normText(a.innerText).length - normText(b.innerText).length;
      })[0];
    if (!hit) return false;
    try { hit.click(); } catch (e2) { return false; }
    return true;
  }

  function fillSignup(values) {
    var nOk = values.name ? fill(nameInput(), values.name) : false;
    var eOk = fill(emailInput(), values.email);
    var pOk = fill(passwordInput(), values.password);
    var parts = (nOk ? "1" : "0") + (eOk ? "1" : "0") + (pOk ? "1" : "0");
    if (eOk && pOk) ok("formFilled", parts);
    else warn("formFilled", parts);
    return eOk && pOk;
  }

  async function stageSignup() {
    if (S.submitted) return; 
    var st = probeSignup();
    if (!st.hasForm) {
      if (once("noform")) note("noForm");
      return;
    }
    clearNote();

    if (!S.answered.signup) {
      ask("signup");
      return;
    }

    if (st.sentHint) return; 

    if (!st.emailOk || !st.pwdOk || !st.nameOk) {
      fillSignup(S.answered.signup);
      await sleep(400);
      st = probeSignup();
      if (!st.emailOk || !st.pwdOk) {
        warn("formIncomplete");
        return;
      }
    }

    if (!S.submitGateAt) {
      S.submitGateAt = Date.now() + FORM_SETTLE_MS;
      S.gateWarned = false;
      saveState();
      note("settle");
    }
    if (S.submitGateAt > 0) {
      var snap = captchaSnapshot();
      if (snap !== S.capSnap) {
        S.capSnap = snap;
        log("capSnapshot", snap);
      }
      if (captchaVerified()) {
        S.submitGateAt = -1; 
        saveState();
        ok("captchaPassed");
      }
    }
    if (S.submitGateAt > 0) {
      if (Date.now() >= S.submitGateAt && !S.gateWarned) {
        S.gateWarned = true;
        warn("captchaPending");
      }
      return;
    }
    if (S.submitGateAt > 0 && Date.now() < S.submitGateAt) return;
    if (Date.now() - S.lastSubmitAt < RETRY_BACKOFF_MS) return;

    if (st.errorHint) warn("submitRejected", st.errorHint);
    if (!st.hasCreate) {
      warn("noCreateBtn");
      S.lastSubmitAt = Date.now();
      saveState();
      return;
    }
    S.lastSubmitAt = Date.now();
    saveState();
    if (!clickText(["创建账号", "注册", "Sign up", "Create account"])) return;
    ok("clickedCreate", st.email);

    var until = Date.now() + 20000;
    var goneStreak = 0;
    while (Date.now() < until && !S.stopped) {
      await sleep(400);
      var after = probeSignup();
      if (after.sentHint) {
        S.submitted = true;
        saveState();
        ok("submitted");
        return;
      }
      if (!after.hasForm) {
        goneStreak += 1;
        if (goneStreak >= 2) {
          S.submitted = true;
          saveState();
          ok("submitted");
          return;
        }
      } else {
        goneStreak = 0;
      }
      if (after.errorHint && after.hasCreate) {
        warn("submitRejected", after.errorHint);
        return;
      }
    }
    warn("submitNoFeedback");
  }

  function stageAskLink() {
    phase("await-email");
    note("awaitEmail");
    ask("link");
  }

  function stageOpenLink() {
    var raw = String(S.answered.link || "");
    S.answered.link = "";
    S.asked.link = false;
    var found = (raw.match(/https?:\/\/[^\s"'<>）)]+/gi) || []).map(function (u) {
      return u.replace(/[.,;]+$/, "");
    });
    var links = found.filter(function (u) {
      try {
        return /(^|\.)chat\.z\.ai$/i.test(new URL(u).hostname);
      } catch (e) {
        return false;
      }
    });
    if (!links.length) {
      fail("linkNotRecognized");
      return false;
    }
    S.link = links[0];
    S.linkOpened = true;
    saveState();
    ok("linkFound", S.link);
    phase("open-link");
    note("openLink");
    navTo(S.link);
    return true;
  }

  async function stageFinish() {
    phase("complete-signup");
    var deadline = Date.now() + FINISH_WAIT_MS;
    var st = probeFinish();
    while (Date.now() < deadline && !st.hasButton && st.pwdCount < 2) {
      await sleep(POLL_MS);
      st = probeFinish();
    }
    if (!st.hasButton && st.pwdCount < 2) {
      if (once("nofinish")) log("noFinishForm");
      await backToAuthorize();
      return;
    }
    clearNote();
    if (S.finishTried) return;
    S.finishTried = true;

    var pw = S.password || (S.answered.signup && S.answered.signup.password) || "";
    if (!pw) {
      note("needPassword");
      await backToAuthorize();
      return;
    }
    var pws = allInputs().filter(function (e) { return e.type === "password"; });
    if (pws.length >= 2) {
      pws.forEach(function (el) { fill(el, pw); });
      await sleep(500);
      if (clickText(["完成注册", "完成", "设置密码", "Finish", "Continue"])) {
        ok("finishSubmitted");
      }
    } else {
      warn("onlyOnePw");
    }
    await sleep(2500);
    if (/注册完成|账户已成功创建|即将跳转|Welcome/i.test(bodyText())) {
      ok("created", S.email);
    }
    await backToAuthorize();
  }

  async function backToAuthorize() {
    if (S.backDone) return;
    S.backDone = true;
    saveState();
    if (!S.returnUrl) {
      warn("noReturnUrl");
      S.stopped = true;
      return;
    }
    phase("back-to-authorize");
    note("backToAuthorize");
    await sleep(300);
    navTo(S.returnUrl);
  }

  async function confirmAuthorize(timeout) {
    var deadline = Date.now() + (timeout || 60000);
    while (Date.now() < deadline) {
      if (onAuthorizePage()) break;
      await sleep(POLL_MS);
    }
    if (!onAuthorizePage()) return false;

    var saidAgree = false;
    for (var i = 0; i < 20 && Date.now() < deadline; i++) {
      if (checkAgreement() && !saidAgree) {
        ok("agreement");
        saidAgree = true;
      }
      await sleep(450);
      if (clickEnabled(["继续", "同意", "确认授权", "授权", "确认", "Continue", "Authorize", "Allow"])) {
        ok("authorizeClicked");
        return true;
      }
      await sleep(600);
    }
    return false;
  }

  async function stageAssist() {

    if (onAuthorizePage()) {
      clearNote();
      phase("confirm-authorize");
      if (!S.authorized) {
        S.authorized = true;
        await confirmAuthorize(45000);
        phase("done");
      }
      return;
    }
    var st = probePhone();
    if (!st.hasForm) return;
    note("assistPhone");
    if (st.phoneOk && st.codeOk) {
      if (st.needAgree) checkAgreement();

      if (st.btnOk && st.code !== S.lastCode && Date.now() - S.lastSubmitAt > 4500) {
        S.lastSubmitAt = Date.now();
        S.lastCode = st.code;
        if (clickText(["登录 / 注册", "登录/注册"])) {
          ok("loginClicked", st.phone, st.code);
        }
      }
    }
  }

  async function tick() {

    if (!pageReady || S.stopped) return;

    if (!S.diagSent) {
      S.diagSent = true;
      var bl = -1;
      var ic = 0;
      try {
        bl = document.body && document.body.innerHTML ? document.body.innerHTML.length : 0;
      } catch (e) {
        bl = -2;
      }
      try {
        ic = allInputs().length;
      } catch (e) {}
      log("diag", document.readyState + " body=" + bl + " inputs=" + ic);
    }

    var page = currentPage();

    if (page !== S.page) {
      S.page = page;

      S.entrySince = 0;
      S.signupClickAt = 0;
      saveState();
      log("page", page, String(location.href).slice(0, 140));
      send({ kind: "page", page: page, url: location.href });
    }

    if (page === "zcode-bridge") {
      S.stopped = true;
      clearNote();
      return;
    }

    if (S.backDone) {
      if (S.mode !== "observe" && onAuthorizePage()) await stageAssist();
      return;
    }

    if (S.answered.link) {
      stageOpenLink();
      return;
    }

    if (S.linkOpened) {
      await stageFinish();
      return;
    }

    if (S.auto && page === "bigmodel-login") {
      phase("unsupported");
      note("bigmodelNoEmail");
      S.mode = "login";
    }

    var probe = probeSignup();
    if (S.auto && (S.submitted || probe.sentHint)) {
      if (probe.sentHint && !S.submitted) {
        S.submitted = true;
        saveState();
        ok("submitted");
      }
      stageAskLink();
      return;
    }

    var looksSignup = probe.hasForm && (probe.hasNameField || probe.hasVerifyBtn || page === "zai-signup");
    if (S.auto && looksSignup) {
      phase("signup-form");
      await stageSignup();
      return;
    }

    if (S.auto && page === "zai-signup") {
      if (!S.signupAt) {
        S.signupAt = Date.now();
        saveState();
      }

      if (Date.now() - S.signupAt > SIGNUP_HYDRATE_MS) {
        if ((S.signupReloads || 0) < SIGNUP_RELOAD_MAX) {
          S.signupReloads = (S.signupReloads || 0) + 1;
          S.signupAt = 0; 
          saveState();
          log("signupReload", String(S.signupReloads), String(location.href).slice(0, 140));
          reloadPage();
          return;
        }
        phase("unsupported");
        note("noForm");
      }
      return;
    }

    if (S.auto) {
      if (!S.entrySince) {
        S.entrySince = Date.now();
        saveState();
      }
      if (S.signupClickAt) {

        if (Date.now() - S.signupClickAt < SIGNUP_CLICK_WAIT_MS) return;
        S.signupClickAt = 0;
        saveState();
      } else if (!S.signupClicked) {
        var entry = findButton(SIGNUP_ENTRY_RE);
        if (entry) {
          S.signupClicked = true;
          S.signupClickAt = Date.now();
          saveState();
          phase("open-signup");
          note("openSignup");
          log("clickSignupEntry", String(location.href).slice(0, 140));

          setTimeout(function () {
            try {
              entry.click();
            } catch (e) {}
          }, 500);
          return;
        }
      }

      if (Date.now() - S.entrySince < SIGNUP_ENTRY_WAIT_MS) return;
      if ((S.signupTries || 0) < 2) {
        S.signupTries = (S.signupTries || 0) + 1;
        saveState();
        phase("open-signup");
        note("openSignup");
        log("navSignupFallback", String(S.signupTries));
        navTo(SIGNUP_URL);
        return;
      }
      phase("unsupported");
      note("noSignupEntry");
      return;
    }

    if (page === "bigmodel-login" || page === "zai-authorize" || page === "zai-auth" || onAuthorizePage()) {
      if (S.mode !== "observe") await stageAssist();
      return;
    }
  }

  function schedule(ms) {
    clearTimeout(S.timer);
    S.timer = setTimeout(function () {
      Promise.resolve()
        .then(tick)
        .catch(function (e) { fail("driverError", e && e.message ? e.message : String(e)); })
        .then(function () { if (!S.stopped) schedule(POLL_MS); });
    }, ms || POLL_MS);
  }

  window.__zpool = {
    state: S,
    fromHost: function (m) {
      if (!m || typeof m !== "object") return;
      if (m.t === "setMode" && m.mode) {
        S.mode = m.mode;

        S.auto = m.mode === "register";
        S.stopped = false;
        S.backDone = false;
        S.asked = {};
        S.submitGateAt = 0;
        S.lastSubmitAt = 0;
        S.wentSignup = false;
        S.lastLogKey = "";
        S.noteKey = "";
        log("modeChanged", m.mode);
        saveState();
        schedule(200);
        return;
      }
      if (m.t === "input" && m.ask) {
        S.answered[m.ask] = m.value;
        if (m.ask === "signup") {
          S.password = (m.value && m.value.password) || "";
          S.email = (m.value && m.value.email) || "";
          S.submitGateAt = 0;
          S.lastSubmitAt = 0;
          log("gotSignup", S.email);
        } else if (m.ask === "link") {
          log("gotLink");
        }
        saveState();
        schedule(200);
        return;
      }
      if (m.t === "action") {
        if (m.action === "retry") {
          S.asked = {};
          S.submitGateAt = 0;
          S.lastSubmitAt = 0;
          S.finishTried = false;
          S.stopped = false;
          S.backDone = false;
          S.signupTries = 0;
          S.submitted = false;
          S.signupAt = 0;
          S.lastLogKey = "";
          S.noteKey = "";
          log("retry");
          saveState();
          schedule(200);
        } else if (m.action === "confirm-authorize") {
          S.mode = "login";
          saveState();
          confirmAuthorize(45000).then(function () { phase("done"); });
        } else if (m.action === "open-signup") {
          S.mode = "register";
          S.wentSignup = true;
          saveState();
          navTo(SIGNUP_URL);
        }
        return;
      }
    }
  };

  send({ kind: "ready", page: currentPage(), url: location.href, mode: S.mode, provider: S.provider });
  schedule(400);
})();
