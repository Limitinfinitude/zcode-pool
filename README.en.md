# Z·POOL (zcode-pool)

[简体中文](README.md) ｜ **English**

A Tauri 2 desktop tool that folds the whole pipeline — **mailbox pool → batch register & verify Z.ai accounts → account switching** — into one app:

- **Mailbox pool**: import an account text file (`email----password----client_id----refresh_token`), keep it locally, and mark each entry *verified / unverified*.
- **Batch auto-verify**: tick the unverified ones and let it run each through Z.ai signup → email verification link → authorization → new account saved.
- **Account library / one-click switch**: switch the login identity between multiple ZCode accounts, with quota shown inline.

Email is read **straight from Microsoft Graph** (using each account's own `client_id` + `refresh_token`) — **no external mail program required**.

## Features

### Mailbox pool

- **Import**: pick an account text file. One account per line, fields separated by `----`:
  ```
  email@outlook.com----password----client_id----refresh_token
  ```
  Only the first four fields are used; an email that already exists is never overwritten — it just counts as *skipped*.
- **Status**: `unverified / verified / failed`. This is **based on local records** — an entry is marked *verified* only when this tool actually completed *signup → email verification → authorization*, not by probing any external state.
- **Direct mail fetch**: exchanges the account's own credentials for a Microsoft token and reads the mailbox newest-first for the latest Z.ai verification link (skipping the non-verification link in the "welcome" email). Rotated refresh tokens are written back automatically.

### Batch auto-verify

Tick the unverified mailboxes → click **Auto-verify** → login windows open one at a time:

- The assistant fills the signup form (random 6-letter nickname, email + password pulled from the pool) and submits;
- **You solve the slider captcha manually once** (Z.ai uses a cloud human-verification check — it can't be bypassed); everything after that is automatic;
- It fetches the verification link from the mailbox, opens it, completes authorization, saves the account, marks the mailbox *verified*, and moves on to the next.

### Account library

- **Save / switch**: switch login identity in one click; the current login is auto-preserved before switching — accounts are never lost.
- **Add account**: OAuth sign-in inside the tool (BigModel / z.ai entry points), with an optional **semi-automatic signup assistant**.
- **Quota**: see each account's plan / quota / expiry right in the list; one-click claim.
- **Import / export**: password-sealed (AES-256-GCM) account bundles for easy migration and backup.

## Building

Requires Node 18+ and Rust.

```bash
npm install
npm run tauri build          # produces src-tauri/target/release/zcode-pool.exe
```

> **Windows note**: this repo's development environment builds with the **GNU** Rust toolchain (the default MSVC toolchain needs a full Windows SDK with Lib/Include, otherwise linking fails — this machine doesn't have it). If you hit MSVC link errors too, use:
> ```bash
> cargo +stable-x86_64-pc-windows-gnu build --release --features tauri/custom-protocol
> ```
> ⚠️ The `--features tauri/custom-protocol` flag is mandatory: without it, Tauri 2 produces a **dev-mode** binary that loads the vite dev server (`127.0.0.1:5173`) at runtime instead of the embedded assets — it shows a blank window / "connection refused".

## Data locations

- Account library / mailbox pool / settings: `~/.zcode-pool/`
- Logs: `%LOCALAPPDATA%\com.zpool.app\logs\oauth.log` (Windows)

## Credits

Parts of this project are based on the following MIT projects; copyright notices are in [LICENSE](LICENSE):

- [zcode-switch](https://github.com/pjpv/zcode-switch) — ZCode account switching
- [OutlookEmail](https://github.com/assast/outlookEmail) — multi-mailbox management (Graph / IMAP)

## License

MIT — see [LICENSE](LICENSE).
