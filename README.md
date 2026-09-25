# Z·POOL (zcode-pool)

**简体中文** ｜ [English](README.en.md)

Z·POOL 是一个基于 **Tauri 2** 的 Windows 桌面工具，用来集中管理 ZCode / Z.ai 多账号、Outlook 邮箱池和账号额度。

![Z·POOL 邮箱库](assets/mailbox.png)

![Z·POOL 账号库 · 额度仪表盘](assets/accounts.png)

## 你可以用它做什么

- **账号库**：保存多个登录身份，一键冷切换，切换前自动保全当前登录。
- **额度仪表盘**：按模型汇总所有账号的剩余额度，并显示套餐、周期、重置时间和有效期。
- **邮箱池**：导入 `email----password----client_id----refresh_token`，读取 Outlook / Hotmail 验证邮件。
- **批量验证**：串行完成注册、取信、验证和授权；滑块验证码仍需手动完成。
- **OAuth 登录**：支持 BigModel 与 z.ai 入口，新增账号不影响当前登录。
- **领取与自动领取**：查看可领取套餐，支持手动领取和定时自动领取。

## 额度统计说明

额度汇总按“**模型名称 + 明细额度**”计算：

- 后端同时提供 `plans[].items` 和顶层 `items` 时，二者是同一份数据，界面只取套餐明细一次。
- 没有套餐明细时，才回退到顶层 `items`。
- 因此总额度不会把同一账号的套餐汇总和明细重复相加。

账号行内仍会保留各套餐的原始明细，方便核对来源。

## 安装与构建

需要 **Node 18+**、Rust stable，以及 Windows GNU 工具链 `stable-x86_64-pc-windows-gnu`。

```powershell
npm install
npm run build
Set-Location src-tauri
cargo +stable-x86_64-pc-windows-gnu build --release --features tauri/custom-protocol
```

生成的 Windows 可执行文件：

```text
src-tauri/target/release/zcode-pool.exe
```

打包便携版和安装包：

```powershell
npm run dist
```

## 数据与安全

- 账号库、邮箱池和设置默认保存在 `~/.zcode-pool/`。
- `mail.json` 包含邮箱密码与 refresh token，请勿提交到 Git 或上传到公共位置。
- 日志位置：
  - Windows：`%LOCALAPPDATA%\\com.zpool.app\\logs\\oauth.log`
  - macOS：`~/Library/Logs/com.zpool.app/`

## 开发检查

```powershell
npm run check:i18n
npm run build
```

## 致谢与许可证

部分代码和设计参考了以下 MIT 项目：

- [zcode-switch](https://github.com/pjpv/zcode-switch)
- [OutlookEmail](https://github.com/assast/outlookEmail)

MIT，详见 [LICENSE](LICENSE)。
