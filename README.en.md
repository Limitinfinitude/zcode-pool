# Z·POOL (zcode-pool)

[简体中文](README.md) ｜ **English**

A Tauri 2 desktop tool that folds **mailbox pool → batch register & verify Z.ai accounts → account switching** into one app.

![screenshot](assets/screenshot.png)

Email is read **straight from Microsoft Graph** (using each account's own `client_id` + `refresh_token`) — **no external mail program required**. The only manual chore in automated signup is the slider captcha; it's a cloud human-verification check that can't be bypassed. Everything else is automatic.

---

## Features

### 📮 Mailbox pool

- **Import**: pick an account text file, one account per line, fields separated by `----`:

  ```
  email@outlook.com----password----client_id----refresh_token
  ```

  Only the first four fields are used; existing emails are never overwritten (they count as *skipped*). You can also **export** the pool back to the same format.
- **Status**: `unverified / verified / failed / needs re-auth`. **Based on local records** — an entry is marked *verified* only when the tool actually completed *signup → email verification → authorization*, never by probing external state.
- **Direct fetch**: exchanges the account's own credentials for a Microsoft token and reads the mailbox newest-first for the latest Z.ai verification link (skipping the non-verification link in the "welcome" email). Rotated refresh tokens are written back automatically.
- **Update credentials**: the row's "✎" lets you paste a fresh line to overwrite it (for when the refresh_token changed).
- **Built-in re-auth**: when a refresh_token is dead, click **Re-authorize** → a **Microsoft device-code flow** (using that mailbox's own client_id) gets a new authorization by entering a code here — **no other tool needed**.

### ✅ Batch auto-verify

Tick the unverified mailboxes → click **Auto-verify** → login windows open **one at a time** (serial):

1. The assistant fills the signup form (random 6-letter nickname, email + password from the pool) and submits;
2. **You solve the slider manually once** in the login window; everything after that is automatic;
3. It fetches the verification link from the mailbox, opens it, completes authorization, saves the account, marks the mailbox *verified*, then moves on.

When done you get a **result list** (who succeeded/failed + reason), and failed ones can be **retried with one click**.

> No concurrency: registering several accounts at once from the same IP trips risk control more easily, and the gain is small (the slider is one-at-a-time anyway). Anyone who wants it can add it themselves.

### 💳 Account library

- **Dashboard**: a **car-dial-style** summary at the top — total remaining tokens plus one dial per model (token count in the centre, turning orange/red when low). Quota is **aggregated per model**, refreshes every 10 minutes, and can be refreshed manually.
- **One-click switch**: switch the login identity between multiple ZCode accounts. Switching is a **cold switch** (close ZCode → swap login → restart, so the device identity takes effect); the current login is auto-preserved first — accounts are never lost.
- **Add account**: OAuth sign-in inside the tool (BigModel / z.ai). Fully manual, **without touching your current login**.
- **Claim**: see each account's plan / quota / expiry inline, with one-click and automatic claiming.

---

## Building

Requires **Node 18+** and **Rust** (stable).

```bash
npm install
npm run tauri build        # output under src-tauri/target/release/
```

Optional installers: `npm run dist` collects a portable exe and the installer into `release/`.

### Per platform

| Platform | Bundle | Notes |
|---|---|---|
| Windows | `nsis` | default; WebView2 runtime is usually present |
| macOS | `dmg` / `app` | `npm run tauri build -- --bundles dmg,app` |
| Linux | `deb` / `appimage` | `npm run tauri build -- --bundles deb,appimage`; needs WebKitGTK and friends |

> **Windows / MSVC link errors?** Tauri 2 uses the `custom-protocol` feature to tell dev from production. If you `cargo build` manually you **must pass it**, otherwise you get a dev binary that loads the vite dev server at runtime (blank window):
> ```bash
> cargo build --release --features tauri/custom-protocol
> ```
> If you hit `LNK1181: cannot open input file 'kernel32.lib'` and the like, the Windows SDK is incomplete; the GNU toolchain sidesteps it: `cargo +stable-x86_64-pc-windows-gnu build --release --features tauri/custom-protocol`.

---

## Data locations

- Account library / mailbox pool / settings: `~/.zcode-pool/`
  - Mailbox pool: `~/.zcode-pool/mail.json` (plain text — holds mailbox passwords and refresh tokens)
  - Accounts: `~/.zcode-pool/accounts/<id>.json`
- Logs: `%LOCALAPPDATA%\com.zpool.app\logs\oauth.log` (Windows) / `~/Library/Logs/com.zpool.app/` (macOS)

---

## Credits

Parts of this project are based on the following MIT projects; copyright notices are in [LICENSE](LICENSE):

- [zcode-switch](https://github.com/pjpv/zcode-switch) — ZCode account switching
- [OutlookEmail](https://github.com/assast/outlookEmail) — multi-mailbox management (Graph / IMAP)

## License

MIT — see [LICENSE](LICENSE).
