# Z·POOL (zcode-pool)

[简体中文](README.md) ｜ **English**

Z·POOL is a **Tauri 2** desktop app for managing ZCode / Z.ai accounts, an Outlook mailbox pool, and account quotas in one place.

![Z·POOL mailbox pool](assets/mailbox.png)

![Z·POOL account library · quota dashboard](assets/accounts.png)

## What it does

- **Account library**: keep multiple login identities and switch between them with a cold switch that preserves the current login first.
- **Quota dashboard**: aggregate remaining quota by model across accounts, with plan, window, reset time, and expiry details.
- **Mailbox pool**: import `email----password----client_id----refresh_token` records and read Outlook / Hotmail verification mail.
- **Batch verification**: run signup, mailbox verification, and authorization serially; slider CAPTCHA still requires a manual step.
- **OAuth sign-in**: add accounts through BigModel or z.ai without changing the current login.
- **Claiming**: inspect available plans and claim them manually or through the scheduled auto-claim flow.

## How quota totals are calculated

Totals are grouped by **model name and quota item**:

- When the backend exposes both `plans[].items` and top-level `items`, they are two views of the same data. The dashboard uses the plan items once.
- If no plan items exist, it falls back to top-level `items`.
- This prevents the same account quota from being counted twice.

The account rows still show the original plan details so you can trace each number.

## Install and build

Requires **Node 18+**, stable Rust, and the Windows GNU toolchain `stable-x86_64-pc-windows-gnu`.

```powershell
npm install
npm run build
Set-Location src-tauri
cargo +stable-x86_64-pc-windows-gnu build --release --features tauri/custom-protocol
```

The Windows executable is written to:

```text
src-tauri/target/release/zcode-pool.exe
```

To collect the portable executable and installer:

```powershell
npm run dist
```

## Data and security

- Account data, mailbox records, and settings live under `~/.zcode-pool/` by default.
- `mail.json` contains mailbox passwords and refresh tokens. Do not commit it to Git or upload it to a public location.
- Logs:
  - Windows: `%LOCALAPPDATA%\\com.zpool.app\\logs\\oauth.log`
  - macOS: `~/Library/Logs/com.zpool.app/`

## Development checks

```powershell
npm run check:i18n
npm run build
```

## Credits and license

Parts of the project reference these MIT projects:

- [zcode-switch](https://github.com/pjpv/zcode-switch)
- [OutlookEmail](https://github.com/assast/outlookEmail)

MIT — see [LICENSE](LICENSE).
