# Z·POOL

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB?logo=tauri&logoColor=white)](https://tauri.app)
[![Release](https://img.shields.io/github/v/release/Limitinfinitude/zcode-pool?label=release)](../../releases)

**简体中文** ｜ [English](README.en.md)

Z·POOL 是一个基于 Tauri 2 的 Windows 桌面工具，用于集中管理 ZCode / Z.ai 的多账号登录身份、Outlook 邮箱池，以及各账号的额度与套餐。

## 界面预览

![邮箱库](assets/mailbox.png)

![账号库与额度](assets/accounts.png)

## 功能特性

### 账号库

- 保存多个登录身份，一键切换，切换前自动存档当前登录，不会丢失账号。
- 支持 BigModel 与 z.ai 两种登录入口，新增账号不影响正在使用的登录。
- 切换后可自动拉起 ZCode，无需手动启动。

### 额度与套餐

- 按模型汇总所有账号的剩余额度，展示套餐、额度周期、重置时间与有效期。
- 支持查询可领取套餐，并可手动领取或按固定间隔自动领取。

### 邮箱池

- 导入 `email----password----client_id----refresh_token` 格式的账号文件，读取 Outlook / Hotmail 验证邮件。
- 批量完成注册、取信、验证与授权流程；滑块验证码需手动完成。
- 支持重新授权、更新凭据、按状态筛选。

## 安装

### 下载预编译版本

前往 [Releases](../../releases) 页面下载对应平台的安装包：

| 平台 | 文件 |
| --- | --- |
| Windows | `*_x64-setup.exe`（安装包） |
| macOS | `*_universal.dmg` |
| Linux | `*.deb` / `*.AppImage` |

程序未做代码签名，首次运行时系统可能提示来源未知。

### 从源码构建

环境要求：Node.js 18 及以上、Rust stable，Windows 平台还需 GNU 工具链 `stable-x86_64-pc-windows-gnu`。

```powershell
npm install
npm run build
Set-Location src-tauri
cargo +stable-x86_64-pc-windows-gnu build --release --features tauri/custom-protocol
```

产物位于 `src-tauri/target/release/zcode-pool.exe`。如需一并生成便携版与安装包：

```powershell
npm run dist
```

修改 i18n 文案后，可用 `npm run check:i18n` 检查中英文词条是否一致。

## 使用说明

1. **添加账号**：在账号库点击「添加」，在弹出的登录窗口中完成登录，账号会自动入库，当前登录不受影响。
2. **切换账号**：在账号列表中点击「切换」。切换会关闭并重启 ZCode，使新的登录状态生效。
3. **批量验证邮箱**：在邮箱库点击「导入 txt」导入账号文件，勾选待验证的邮箱，点击「自动验证」；每个账号需要手动完成一次滑块验证。
4. **领取套餐**：点击「刷新资格」查看各账号可领取的套餐，随后手动「领取」，或开启「自动领取」按固定间隔检查。

## 数据与安全

- 账号库、邮箱池与设置均保存在 `~/.zcode-pool/` 目录，全部为本地数据。
- `mail.json` 以明文保存邮箱密码与 refresh token，请勿提交至版本库或对外分享。
- 导出账号时仅包含 z.ai / BigModel 的登录凭据、供应商配置与设备标识；第三方供应商密钥、SSH 密码及中继凭据会被剔除。
- 运行日志位于 `%LOCALAPPDATA%\com.zpool.app\logs\oauth.log`。

## 免责声明

本项目仅用于技术分享与学习交流，请遵守相关服务条款与法律法规，自行承担使用风险。

## 致谢

部分代码与设计参考了以下 MIT 项目：

- [zcode-switch](https://github.com/pjpv/zcode-switch)
- [OutlookEmail](https://github.com/assast/outlookEmail)

## 许可证

[MIT](LICENSE)
