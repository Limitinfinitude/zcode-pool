/*
 * Z·POOL 登录窗口注册助手驱动。
 *
 * 这份脚本由 Rust 侧通过 initialization_script 注入到 login 这个 webview，
 * 每次页面加载（含跨域跳转）都会在最早期重新执行一遍。它只在这个窗口里跑，
 * 主窗、设置窗、验证码窗都不受影响。
 *
 * 分工（半自动）：
 *   人来做 —— 收短信 / 过人机验证 / 去邮箱复制激活链接 / 提供名称邮箱密码；
 *   驱动来做 —— 识别页面、填表、在合适的时候点提交、打开激活链接、确认 OAuth 授权。
 *
 * 与宿主的两条通道：
 *   驱动 -> 宿主：window.location.href = "zpool-driver://msg?..."，
 *                 由 Rust 的 on_navigation 拦下（同 zcode://oauth/callback 的做法）。
 *   宿主 -> 驱动：window.__zpool.fromHost({...})，由 Rust 侧 eval 调用。
 *
 * 文案一律不发中文/英文，只发「码」（phase / code / ask）+ 动态参数，
 * 由主窗按当前语言渲染，这样中英双语是全的。
 *
 * 驱动不持有任何取号 / 收信通道，也不读取、不上报页面上的凭据。
 */
(function () {
  "use strict";
  if (window.__zpool) return;
  // 只对顶层文档生效：这个脚本会注入到所有 frame，而页面里的验证码 iframe
  // 同样是跨域的——在里面发哨兵跳转不仅传不回来（on_navigation 只看主框架），
  // 还可能把那个 iframe 本身搞坏。
  if (window.top !== window.self) return;

  // 注：这里曾经挂过一层 `window.initAliyunCaptcha` 钩子，想复用领取那边的
  // 「无痕验证」（startTracelessVerification）绕开注册页滑块。实测**此路不通**，
  // 已整段移除，原因记在下面，免得后人再走一遍：
  //   1. 注册页和领取根本不是同一个阿里云场景 —— 注册页 SceneId=`36qgs6xb`、region=`sgp`；
  //      领取是 `11xygtvd`、region=`cn`。场景不同，能力不能互相借。
  //   2. 注册页用的是 **embed 模式**，而 embed 下 startTracelessVerification() 是空操作，
  //      Promise resolve 出 undefined，压根不产生验证参数。
  //   3. 想另起一个隐藏的 popup 实例去跑无痕 —— 同一个场景拿不到参数（tracelessGot=empty），
  //      而且 SDK 只允许一个实例，**第二个 init 会把页面自己的滑块顶掉**，反而无法验证。
  // 结论：注册页的验证参数只能由页面自己的滑块产生，老老实实让用户滑。

  var CFG = window.__ZPOOL_CFG || {};
  var POLL_MS = 900;
  // 表单填满后的**兜底**等待上限。正常不该等满：滑块判定有信号就立刻提交。
  var FORM_SETTLE_MS = 20000;
  var RETRY_BACKOFF_MS = 12000;
  var FINISH_WAIT_MS = 25000;
  var SIGNUP_URL = "https://chat.z.ai/auth?action=signup";
  var STORE_KEY = "zpool-driver-state";
  // 注册页自愈：驱动用 location.href 强跳 chat.z.ai/auth?action=signup 时，
  // 实测落地的文档是「没水合的骨架屏」（body 约 3.3K、一个 input 都没有），
  // 而 location.reload() 一次就必然出水合好的表单（body 约 11K）。
  // 所以在注册页等不到表单就自动刷新，最多 2 次。
  var SIGNUP_HYDRATE_MS = 4500;
  var SIGNUP_RELOAD_MAX = 2;
  // 注册模式切页策略：优先「点」登录页上的注册入口，让 SPA 自己走客户端路由。
  // 实测用 location.href 强跳 ?action=signup 是冷加载，落地经常是「没水合的骨架屏」
  // （body 3.3K~7K、一个表单控件都没有，反复刷新也是 7K 原地踏步），
  // 而人工流程（点注册）稳定出表单 —— 因为资源已加载、SPA 状态都在。
  var SIGNUP_ENTRY_RE = /^(注册|免费注册|立即注册|创建账号|Sign up|Create account|Register)$/;
  // 等「注册」入口出现，同时也是在等登录页把 SPA 的懒加载 chunk 装完。
  // 实测：登录页 ready 后**立刻**跳注册页必挂（骨架屏，刷两次也救不回），
  // 等 ~6 秒以上再切就稳 —— 所以这个窗口是承重的，别随意调小。
  var SIGNUP_ENTRY_WAIT_MS = 9000; // 等注册入口渲染出来的上限
  var SIGNUP_CLICK_WAIT_MS = 6000; // 点完之后等表单渲染的上限

  var S = {
    mode: CFG.mode || "observe",
    /**
     * 是否允许**自动跑注册那一套**（跳注册页 / 填表 / 要激活链接）。
     * 只有「批量自动验证」这条流程会给 true（Rust 侧 oauth_begin 的 batch 参数）。
     * 手动添加一律 false —— 不管点的是注册还是登录，驱动只旁观。
     * 抽屉里的「改用邮箱注册」按钮是唯一的显式开关（走 setMode）。
     */
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
    /** 已经跳过一次注册页，避免打完激活链接又被打回去 */
    wentSignup: false,
    /** 为了到注册表单一共跳了几次（跨页面要保留，否则会来回弹） */
    signupTries: 0,
    /** 注册已提交成功（看到「验证邮件已发送」或提交后表单消失）—— 一旦置位绝不再回注册页 */
    submitted: false,
    /** 第一次落到注册页的时间，用来判断表单是不是根本出不来 */
    signupAt: 0,
    /** 激活链接已经在本窗口打开 */
    linkOpened: false,
    /** 已经回到（或尝试回到）授权页 */
    backDone: false,
    lastSubmitAt: 0,
    lastCode: "",
    submitGateAt: 0,
    finishTried: false,
    authorized: false,
    stopped: false,
    lastLogKey: "",
    noteKey: "",
    /** 注册页等不到表单时，已经自动刷新了几次 */
    signupReloads: 0,
    /** 是否已经点过登录页上的「注册」入口 */
    signupClicked: false,
    /** 点「注册」入口的时间：用来判断点完多久还没出表单 */
    signupClickAt: 0,
    /** 落到当前非注册页的开始时间：用来给「等注册入口出现」计时 */
    entrySince: 0,
    /** 上一次报过的验证码现场快照（变了才报，免得刷屏又干扰页面） */
    capSnap: "",
    /** 宽限期到了但滑块还没过，只提醒一次（提醒而已，不放行） */
    gateWarned: false,
    timer: 0
  };

  /*
   * 这个脚本每次页面加载都会重跑一遍（initialization_script 的语义），
   * 而注册流程中间必然要跨页面跳转（→ 注册页、→ 激活链接、→ 授权页）。
   * 所以流程状态必须自己存起来，否则「打开激活链接」那一步之后状态就没了，
   * 新文档会把人又推回注册表单。
   *
   * 用 sessionStorage：同源跳转都在 chat.z.ai 内，天然够用；
   * 而每条流程用的是全新的 webview profile 目录，存储天生是空的，
   * 不会把上一条流程的状态带过来。
   */
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
      /* 存储被禁用：退化成不持久，单页内的流程不受影响 */
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

  // ------------------------------------------------------------ 基础工具

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

  /*
   * 页面 -> 宿主只能靠「跳到一个自定义 scheme，由 on_navigation 拦下」，
   * 这和仓库原本处理 zcode://oauth/callback 是同一机制。
   *
   * 但 timing 很要命：在 document-start 阶段顶掉正在进行的加载，
   * 即使导航被拦下，渲染进程也已经把这次加载挂断了 —— 结果就是
   * 地址栏/URL 是对的、文档却是空的（黑屏、读不到任何表单）。
   * 所以这里做了两件事：
   *   1. readyState 还是 loading 时只入队，不发；
   *   2. 加载完成后再按间隔一条条发，避免连续导航互相打断。
   */
  var pageReady = document.readyState !== "loading";
  var outbox = [];
  var flushing = false;
  var flushTimer = 0;
  var retryTimer = 0;
  /** 已经决定真正跳走了：此后不再有任何回传跳转，免得把导航顶掉 */
  var halted = false;

  function go(url) {
    try {
      window.location.href = url;
    } catch (e) {}
  }

  /**
   * 强制整页重新加载（注册页骨架屏没水合时的自愈）。
   *
   * 和 navTo 一样：先把回传队列排空、停掉定时器再刷 —— 回传用的 location.href
   * 和真实导航是同一个通道，排在后面的回传会把这次刷新顶掉。
   * 刷完 4 秒脚本还活着，说明这次刷新没落地，再重试两次，别让驱动从此哑掉。
   */
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

  /**
   * 真正跳到一个 http(s) 页面（注册页 / 激活链接 / 授权页）。
   *
   * 这里必须先把队列排空并停掉定时器：回传用的是 location.href，而它和真实
   * 跳转是同一个通道 —— 排在后面的那次回传会把这次跳转直接顶掉，页面就永远
   * 切不过去。这个坑是实测踩出来的。
   *
   * 再加一层自愈：4 秒后如果脚本还活着，说明这次跳转没落地（被顶掉或失败），
   * 就重推一次；连续几次都不行才报错，不让人干等。
   */
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
          halted = false; // 恢复回传，把失败原因告诉面板
          fail("navStuck", url);
          return;
        }
        go(url);
        retryTimer = setTimeout(retry, 4000);
      }
      retryTimer = setTimeout(retry, 4000);
    }, 80);
  }

  // 注意：这里**不能**用 beforeunload 来"页面要卸载就闭嘴"。
  // 回传本身就是一次 location.href 跳转，实测它会连带触发本页的
  // beforeunload —— 于是第一条消息发出去之后 halted 就被自己置位，
  // 后面所有消息被 send() 丢掉（表现：抽屉里只有一条「已连接登录窗口」，
  // 页面/自检/阶段全都没有，但页面其实已经切过去了）。
  // 真正需要闭嘴的时刻集中在 navTo() 里，那里先 drain 再 halt，够了。

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
    // 兜底：万一 DOMContentLoaded 被别的脚本打断，也不能永远闭嘴
    setTimeout(markReady, 4000);
  }

  /**
   * 过程日志：只发码 + 动态参数，文案在主窗。
   *
   * 轮询是每 900ms 跑一遍的，同一个状态会被反复看到；这里做「连续重复去重」，
   * 否则每轮都会白白发一次哨兵跳转。
   */
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

  /** 需要人工介入的提示条；code = "clear" 收起。同样只发变化量。 */
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

  // Vue/React 受控输入：必须走原生 setter 再补发事件，否则框架状态不更新。
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

  // 和 clickText 一样找按钮，但要求它没被禁用 —— 授权页的「继续」在勾协议前是灰的，
  // 点它没用，得等它变可用。
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

  // ------------------------------------------------------------ 页面识别

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

  // ------------------------------------------------------------ 表单探测

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

  /**
   * 昵称/名称输入框（严格匹配 placeholder）。
   * 登录页**没有**这个字段，注册表单有 —— 用来把「登录页」和「注册表单」分开。
   * 不能用 nameInput()：它的兜底会匹配任意 text 输入框，登录页上也可能是真。
   */
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
      /** 注册表单特有的昵称框（登录页没有） */
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

  // 勾同意协议。各站点组件五花八门（原生 checkbox / Element UI / antd / 自绘
  // role=checkbox），逐个兜底；实在找不到输入框，就点「用户协议」那段文字本身。
  function agreementBoxes() {
    var sel = 'input[type="checkbox"], .el-checkbox__original, .ant-checkbox-input, [role="checkbox"]';
    return Array.prototype.slice.call(document.querySelectorAll(sel)).filter(vis);
  }

  /**
   * 滑块是否已通过。多判据、任一命中即算：
   *   1. 页面上出现「验证通过 / 验证成功 / Verified」文案；
   *   2. 隐藏域里出现了 captcha / verify / token 之类的长值（阿里云组件通过后写入）。
   * 都不命中就返回 false —— 退回原来的「等宽限期 + 被拒退避」，宁可慢也别误判。
   */
  /**
   * 把主文档 + 开放的 shadow root + 同源 iframe 里的可见文字拼起来。
   *
   * 阿里云验证码控件常把 UI 渲染在 shadow root / iframe 内，
   * `document.body.innerText` 看不到 —— 这正是「滑块明明过了却判不出来、
   * 只能干等满 12 秒」的原因。跨域 iframe 读不到（正常），交给隐藏域兜底。
   */
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

  /** 主文档 + 开放 shadow root 里的隐藏域 / 文本域（验证码 token 兜底判断用） */
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

  /**
   * 验证码现场快照（诊断用）。判不出来时靠它定位卡在哪一环：
   *   ok  —— 文字判据命中没
   *   nc  —— 阿里云控件容器数（nc_ / aliyun）
   *   sh  —— 页面里开放 shadow root 数（>0 说明深读有意义）
   *   ifr —— iframe 数（跨域读不到，只能靠 fL 兜底）
   *   fL  —— 名字像验证码、且值够长的隐藏域数
   */
  /** 「创建账号」按钮状态：验证码校验期间站点一般会禁掉它，放开即代表校验完成 */
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
    var fC = 0; // 名字像验证码、且值够长的域
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
    // 自绘组件：点「用户协议 / 隐私政策」文字所在的 label / 容器。
    // 取文字最短的那个命中项，避免点到把整段包住的大容器。
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

  // ------------------------------------------------------------ 各阶段

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
    if (S.submitted) return; // 已提交，剩下的交给 stageAskLink
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

    if (st.sentHint) return; // 交给 stageAskLink

    if (!st.emailOk || !st.pwdOk || !st.nameOk) {
      fillSignup(S.answered.signup);
      await sleep(400);
      st = probeSignup();
      if (!st.emailOk || !st.pwdOk) {
        warn("formIncomplete");
        return;
      }
    }

    // 提交闸门：填满后先给一个宽限期让人过滑块，但**边等边检测**——
    // 一旦识别到「验证已通过」就立刻提交，不再干等固定的 12 秒。
    // 阿里云组件的 DOM 会随框架重渲染整体消失又重建，所以不依赖单一判据：
    // 文案 / 隐藏 token 域，任一命中即算过；都没有就退回「等宽限期 + 被拒退避」。
    if (!S.submitGateAt) {
      S.submitGateAt = Date.now() + FORM_SETTLE_MS;
      S.gateWarned = false;
      saveState();
      note("settle");
    }
    if (S.submitGateAt > 0) {
      // 现场快照：只在变化时上报（免得刷屏、也免得回传把验证码控件顶掉）。
      var snap = captchaSnapshot();
      if (snap !== S.capSnap) {
        S.capSnap = snap;
        log("capSnapshot", snap);
      }
      // 判据只有一条：文字/隐藏域命中「验证通过」。
      // 试过「提交按钮从禁用到放开」，实测站点**从不**禁按钮（两次失败流程里
      // b=on bd=0 全程如此），这条判据无效，已删。
      if (captchaVerified()) {
        S.submitGateAt = -1; // 过了：立刻提交
        saveState();
        ok("captchaPassed");
      }
    }
    // 滑块没判定通过就**绝不提交**。实测提前交上去会把整条流程搞乱
    // （表单被顶回登录页、后续几个号都得手点）。宁可在这儿等用户过滑块 ——
    // 宽限期只用来上报状态，不用来放行。
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

    /*
     * 点完立刻原地盯 20 秒。
     *
     * 「验证邮件已发送」多半是个几秒就消失的 toast，而 tick 是 900ms 一轮 ——
     * 等下一轮再看经常已经错过，于是被判成「没提交」，又跳回注册页重来一遍
     * （重来会拿新的验证令牌，把上一封邮件里的链接作废，用户就收不到可用的信了）。
     * 这里用 400ms 的粒度盯着，看到成功提示立刻落成持久标志并收工。
     */
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
      // 表单消失且没报错 —— 多半就是切到「去查收邮件」了。
      // 要求连续两次都消失，避免被一次重渲染骗到。
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
    // 先勾协议、再点「继续」。按钮在勾选前是 disabled 的，勾上后组件还要一拍才
    // 变可用；重渲染偶尔还会把勾选吞掉，所以多试几轮，且只点「可用」的按钮。
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
    // 登录协助：手机号表单在两项都填好后自动点「登录 / 注册」，
    // 落到授权页时自动勾协议 + 点「继续」。
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
      // 只有验证码变过才重提：填错时反复原样提交最容易招风控
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
    // 页面还没加载完就别做事：这时候 DOM 还没成形，任何判断都是错的，
    // 而且还会平白无故把页面导走。
    if (!pageReady || S.stopped) return;

    // 每个文档只报一次自检，黑屏 / 空白页这类问题靠它定位
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
      // 换页了：等注册入口的计时、点完等表单的计时都要重来
      S.entrySince = 0;
      S.signupClickAt = 0;
      saveState();
      log("page", page, String(location.href).slice(0, 140));
      send({ kind: "page", page: page, url: location.href });
    }

    // zcode.z.ai 那个中转页会自己跳到 zcode://oauth/callback，
    // 驱动在这里必须完全闭嘴，别去打扰它。
    if (page === "zcode-bridge") {
      S.stopped = true;
      clearNote();
      return;
    }

    // 回到授权页之后，注册流程就此打住：只剩最后一件事 —— 勾协议 + 点「继续」。
    // 这一步必须单独走并立刻 return：不能再落回下面的注册分支，否则 S.submitted
    // 会把状态又推去「取激活链接」，卡成死循环（这就是当初加 backDone 的原因）。
    if (S.backDone) {
      if (S.mode !== "observe" && onAuthorizePage()) await stageAssist();
      return;
    }

    // 1) 激活链接已到手：优先打开
    if (S.answered.link) {
      stageOpenLink();
      return;
    }

    // 2) 激活链接已打开：走「完成注册」这一步
    if (S.linkOpened) {
      await stageFinish();
      return;
    }

    // 3) BigModel 没有邮箱注册入口，别把它往死路上带
    if (S.auto && page === "bigmodel-login") {
      phase("unsupported");
      note("bigmodelNoEmail");
      S.mode = "login";
    }

    // 4) 提交结果优先判 —— 必须放在表单判断**之前**。
    //    提交成功后页面会切到「验证邮件已发送」，表单随之消失；
    //    如果先要求「找到邮箱框+密码框」，成功提示这条分支永远走不到，
    //    就会掉进下面的兜底里又跳回注册页、又让人填一遍（实测踩到）。
    // 从这里往下都是**注册自动化**，只允许在 register 模式下跑。
    //   批量验证 = register；手动「添加」= observe；登录协助 = login。
    // 以前这里会把「看起来像注册表单」的页面直接接管，还顺手把模式改成 register ——
    // 而**登录页也满足 looksSignup**（同样有邮箱+密码，同样有那个「点击开始验证」按钮），
    // 结果在登录页点一下就被人推去注册、弹出填表框和粘贴链接框，手动加一个已经注册
    // 好的号根本做不成。手动流程要的是「什么都不做」，所以这里只认模式、不再自作主张。
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

    // 5) 注册表单：以 DOM 为准，不以 URL 为准。
    //    SPA 完全可能把内容换成注册表单而地址还停在 /auth，
    //    只认 URL 会出现「明明在注册页却被当成登录页导走」。
    // 别用 hasCreate 判：登录页同样有「邮箱+密码」和一个「注册」入口，
    // 实测登录页水合后（body≈13.6K / inputs=2）会被误判成 signup-form，直接在登录页填表。
    // 只认注册表单特有的特征：昵称框 / 滑块验证按钮 / URL 就是注册页。
    var looksSignup = probe.hasForm && (probe.hasNameField || probe.hasVerifyBtn || page === "zai-signup");
    if (S.auto && looksSignup) {
      phase("signup-form");
      await stageSignup();
      return;
    }

    // 6) 已经在注册页：原地等表单出来，**不要再跳**（同样只归注册模式管 ——
    //    observe 下手动加号时，这个自刷会去 reload 用户正在操作的页面）
    if (S.auto && page === "zai-signup") {
      if (!S.signupAt) {
        S.signupAt = Date.now();
        saveState();
      }
      // 驱动用 location.href 强跳过来的那份文档常常是「没水合的骨架屏」
      // （实测 body≈3.3K、inputs=0，干等 12 秒也不出表单）；手动刷新一次必好，
      // 这里就自动刷 —— 最多 SIGNUP_RELOAD_MAX 次，还出不来才判 unsupported。
      if (Date.now() - S.signupAt > SIGNUP_HYDRATE_MS) {
        if ((S.signupReloads || 0) < SIGNUP_RELOAD_MAX) {
          S.signupReloads = (S.signupReloads || 0) + 1;
          S.signupAt = 0; // 归零：新文档重新起算等待窗口，别白白多刷一次
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

    // 7) 还没到注册页：切过去。
    //
    // 首选「点」登录页上的注册入口 —— SPA 客户端路由，JS 资源已加载、状态都在，
    // 表单稳定渲染（人工流程就是这么走的）。location.href 强跳是冷加载，落地经常是
    // 没水合的骨架屏（body 3.3K~7K、一个表单控件都没有），只能当兜底。
    if (S.auto) {
      if (!S.entrySince) {
        S.entrySince = Date.now();
        saveState();
      }
      if (S.signupClickAt) {
        // 点过了：等表单渲染，超时才往下走兜底
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
          // 回传走的是 location.href 跳转，和 SPA 的客户端路由抢同一个通道：
          // 等这几条回传发完（各 160ms 间隔）再点，别把刚切出来的表单顶掉。
          setTimeout(function () {
            try {
              entry.click();
            } catch (e) {}
          }, 500);
          return;
        }
      }
      // 入口还没渲染出来就继续等着 —— 页面水合要时间，别一上来就冷加载
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

    // 6) 登录协助
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

  // ------------------------------------------------------------ 宿主入口

  window.__zpool = {
    state: S,
    fromHost: function (m) {
      if (!m || typeof m !== "object") return;
      if (m.t === "setMode" && m.mode) {
        S.mode = m.mode;
        // 「改用邮箱注册」是手动流程里唯一的显式开关：点了才开自动化。
        // 切回 observe / login 就关掉。
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
