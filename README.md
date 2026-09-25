# Z·POOL (zcode-pool)

**简体中文** ｜ [English](README.en.md)

Tauri 2 桌面工具。把「邮箱池 → 批量注册验证 Z.ai 账号 → 账号切换」这条链路收进一个 app：

- **邮箱库**：导入账号文本（`email----password----client_id----refresh_token`），本地建池，标出「已验证 / 未验证」。
- **批量自动验证**：勾选未验证的号，逐个走完 Z.ai 注册 → 取邮箱验证链接 → 完成授权 → 新账号入库。
- **账号库 / 一键切换**：在多个 ZCode 账号之间切换登录身份，自动显示额度。

读信**直连微软 Graph**（用账号自带的 `client_id` + `refresh_token`），**不依赖任何外部邮箱程序**。

## 功能

### 邮箱库

- **导入**：选一个账号文本文件。每行一个账号，字段用 `----` 分隔：
  ```
  email@outlook.com----password----client_id----refresh_token
  ```
  只取前四段；已存在的邮箱不覆盖，只计「跳过」。
- **状态**：`未验证 / 已验证 / 失败`。**以本地记录为准**——只有本工具真的跑通「注册 → 邮箱验证 → 授权入库」才置「已验证」，不查外部状态。
- **直连取信**：用账号自带的凭据直接和微软换 token 读邮件，按时间倒序找最新的 Z.ai 验证链接（跳过「欢迎邮件」里那种非验证链接）。令牌轮换时自动回写。

### 批量自动验证

勾选未验证的号 → 点「自动验证」→ 逐个弹登录窗：

- 助手自动填注册表单（昵称随机 6 位、邮箱密码取自邮箱池）、自动提交；
- **滑块验证需要你手动过一次**（Z.ai 用的是云端人机校验，绕不过）——过完其余全自动；
- 自动从邮箱取验证链接并打开、自动完成授权、新账号入库、把该邮箱标「已验证」，然后跑下一个。

### 账号库

- **保存 / 切换**：一键切换登录身份；切换前自动保全当前登录，绝不丢号。
- **添加账号**：工具内 OAuth 登录新号（BigModel / z.ai 双入口），可选**注册助手**做半自动注册。
- **额度**：列表里直接看每个账号的订阅 / 额度 / 有效期；支持一键领取。
- **导入 / 导出**：加密（口令 + AES-256-GCM）打包账号，便于迁移备份。

## 构建

需要 Node 18+ 与 Rust。

```bash
npm install
npm run tauri build          # 产出 src-tauri/target/release/zcode-pool.exe
```

> **Windows 注意**：本仓库的开发环境用 **GNU** Rust 工具链构建（`cargo build` 默认的 MSVC 工具链需要完整安装 Windows SDK 的 Lib/Include，否则链接必失败；本机没装）。若你也遇到 MSVC 链接失败，改用：
> ```bash
> cargo +stable-x86_64-pc-windows-gnu build --release --features tauri/custom-protocol
> ```
> ⚠️ 必须带 `--features tauri/custom-protocol`：Tauri 2 里不带它编出来的是**开发模式**二进制，运行时会去连 vite dev server（`127.0.0.1:5173`）而不是内嵌资源，表现为白屏 / 「拒绝连接」。

## 数据位置

- 账号库 / 邮箱池 / 设置：`~/.zcode-pool/`
- 日志：`%LOCALAPPDATA%\com.zpool.app\logs\oauth.log`（Windows）

## 致谢

本项目的部分代码与设计参考了以下 MIT 项目，版权声明见 [LICENSE](LICENSE)：

- [zcode-switch](https://github.com/pjpv/zcode-switch) —— ZCode 账号切换
- [OutlookEmail](https://github.com/assast/outlookEmail) —— 多邮箱管理（Graph / IMAP）

## 许可证

MIT，见 [LICENSE](LICENSE)。
