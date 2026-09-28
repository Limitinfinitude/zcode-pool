# Z·POOL

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB?logo=tauri&logoColor=white)](https://tauri.app)
[![Release](https://img.shields.io/github/v/release/Limitinfinitude/zcode-pool?label=release)](../../releases)

[简体中文](README.md) ｜ **English**

Z·POOL is a Tauri 2 desktop application that keeps ZCode / Z.ai account identities, an Outlook mailbox pool and
per-account quotas in one place — and exposes a local proxy with a **web console** so third-party apps such as
pi-agent or LobeChat can spend the pool's quota directly.

## Screenshots

**Web console** (open `http://127.0.0.1:8899/proxy`; page edits need no rebuild)

| Dashboard | Channel health |
| :---: | :---: |
| ![Dashboard](assets/dashboard.png) | ![Channel health](assets/channels.png) |

| Usage | Account pool |
| :---: | :---: |
| ![Usage](assets/usage.png) | ![Account pool](assets/accounts-console.png) |

**Desktop panel** (mailbox pool / account library)

| Mailbox pool | Account library and quotas |
| :---: | :---: |
| ![Mailbox pool](assets/mailbox.png) | ![Account library and quotas](assets/accounts.png) |

## Features

### proxy gateway + web console

- Serves an Anthropic-protocol endpoint (`http://127.0.0.1:8899/v1/messages`) that clients can point straight at the pool.
- **Ships its own console**: relay on/off, listen address, port, API keys, account-selection policy, model policy and
  model mapping are all editable from the browser — no need to go back to the desktop panel.
- **Switchable listen address**: loopback only by default. Binding to a LAN-reachable address **requires at least one
  API key first**, otherwise anyone on the same subnet can burn your quota. Auth rule: loopback is exempt,
  remote callers must present a key.
- **Model policy**: `auto fallback` (when the requested model has no quota anywhere, use another one that does) or
  `pinned` (lock one model; no switching even when exhausted).
- **Model mapping**: rename or retire a model at the gateway instead of editing every client.
- **Quota cache on disk**: quotas no longer live only in memory, so a restart doesn't re-query everything.
- **Automatic captcha minting**: upstream demands a one-shot verification parameter on every request; the app keeps
  generating them in the background. Nothing to babysit.

### Console pages

| Page | What it does |
| --- | --- |
| Dashboard | Service state, currently routed account, pool throughput, TTFB / total latency, 60-sample sparkline, probe-latency and request-outcome distributions |
| Channels | One row per account: state, latency, recent-history sparkline, success rate, failure streak, last probe; probe / unfreeze per row, plus a pool-wide latency summary (mean / median / P95 / sample count) |
| Usage | Today's and lifetime tokens (input / output / cache broken out), success rate and cache-hit share, grouping by model / account / key, filterable and paginated request log |
| Accounts | One row per account with status and name filters plus paging; ring-gauge quota dashboard, quotas expiring today, sync-from-login, launch / kill ZCode |
| Mailbox | Import / export the mailbox pool, batch verification, status filter |
| Settings | Relay toggle / listen address / port / API keys / captcha pool / ZCode path / selection policy / model policy / model mapping |
| Logs | Raw runtime log with paging |

The console also serves a **chat page** (`http://127.0.0.1:8899/`) for verifying the relay end to end, with streaming
output and collapsible reasoning.

### Account library

- Store multiple login identities and switch between them in one click. The current login is archived before every
  switch, so no account is ever lost.
- Supports both BigModel and z.ai sign-in. Adding an account never disturbs the login you are using.
- Can relaunch ZCode automatically after a switch.

### Quotas and plans

- Aggregates remaining quota across all accounts, grouped by model, with plan, quota window, reset time and expiry.
- Checks which plans are claimable, and claims them manually or on a fixed schedule.

### Mailbox pool

- Imports `email----password----client_id----refresh_token` files and reads Outlook / Hotmail verification mail.
- Runs signup, mail pickup, verification and authorization in batches. The slider CAPTCHA is completed manually.
- Supports re-authorization, credential updates and status filtering.

## Installation

### Prebuilt binaries

Download the package for your platform from the [Releases](../../releases) page:

| Platform | File |
| --- | --- |
| Windows | `*_x64-setup.exe` |
| macOS | `*_universal.dmg` |
| Linux | `*.deb` / `*.AppImage` |

The binaries are not code-signed, so the system may warn about an unknown publisher on first launch.

### Building from source

Requirements: Node.js 18 or newer, stable Rust, and — on Windows — the GNU toolchain `stable-x86_64-pc-windows-gnu`.

```powershell
npm install
npm run build
Set-Location src-tauri
cargo +stable-x86_64-pc-windows-gnu build --release --features tauri/custom-protocol
```

The binary is written to `src-tauri/target/release/zcode-pool.exe`. To also produce the portable build and the installer:

```powershell
npm run dist
```

After editing i18n strings, `npm run check:i18n` verifies that the Chinese and English tables stay in sync.

## Usage

1. **Add an account** — in the account library, click *Add* and sign in through the window that opens. The account is stored automatically and your current login is untouched.
2. **Switch accounts** — click *Switch* on a row. ZCode is closed and restarted so the new login takes effect.
3. **Verify mailboxes** — in the mailbox pool, click *Import txt*, tick the mailboxes you want, then click *Auto-verify*. Each account needs one manual slider check.
4. **Claim plans** — click *Refresh* to see what each account can claim, then *Claim* manually or enable *Auto claim* to check on a fixed interval.
5. **Point a client at it** — confirm the relay is on in Settings, then use `http://127.0.0.1:8899` as the base URL (Anthropic protocol).
   Loopback callers need no key; to reach it from another machine on the LAN, switch the listen address and create an API key.

## Data and security

- Accounts, mailboxes and settings live under `~/.zcode-pool/`. Everything stays local.
- `mail.json` stores mailbox passwords and refresh tokens in plain text. Do not commit it or share it.
- Exported accounts contain only z.ai / BigModel credentials, provider config and the device identity. Third-party provider keys, SSH passwords and relay credentials are stripped.
- The relay listens on `127.0.0.1` only by default. Binding it to a LAN-reachable address **requires** at least one API key, or the listener is refused.
- Usage records keep only the first 8 characters of an API key — never the full secret.
- Logs are written to `%LOCALAPPDATA%\com.zpool.app\logs\oauth.log`.

## Disclaimer

This project is shared for technical learning only. Please comply with the relevant terms of service and laws, and use it at your own risk.

## Credits

Parts of the code and design reference these MIT projects:

- [zcode-switch](https://github.com/pjpv/zcode-switch)
- [OutlookEmail](https://github.com/assast/outlookEmail)

## License

[MIT](LICENSE)
