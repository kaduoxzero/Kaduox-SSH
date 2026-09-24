# E2E / 视觉回归测试

基于 tauri-driver + WebdriverIO + WebView2，驱动真实桌面应用窗口；截图基线用 pixelmatch 比对（阈值 0.5%）。

## 前置条件

1. 仓库根目录 `tools/` 下需要驱动（已在 .gitignore 排除，首次需安装）：
   - `cargo install tauri-driver --locked --root <repo>/tools`
   - `tools/edgedriver/msedgedriver.exe`，版本需与本机 WebView2 Runtime 一致
     （查版本：注册表 `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}` 的 `pv`）
2. 构建被测应用（必须 `tauri build --debug --no-bundle`，直接 `cargo build` 的 debug 包会去连 vite dev 服务器）：
   - `npx tauri build --debug --no-bundle`
3. E2E-02~07 依赖主机库中存在别名 `本地机器`（127.0.0.1 loopback OpenSSH）且系统 keyring 已存其密码。

## 运行

```powershell
npm run test:e2e
```

## 更新视觉基线

UI 预期变更后重建基线：

```powershell
$env:UPDATE_BASELINE='1'; npm run test:e2e; Remove-Item Env:\UPDATE_BASELINE
```

基线提交在 `e2e/baseline/`；失败时的实际图与 diff 图在 `e2e/artifacts/`（不入库）。

## 设计约定

- 终端 `.xterm` 区域（动态内容：光标闪烁、渲染噪声）在视觉比对中涂黑屏蔽，终端正确性改用文本断言。
- 终端输入链路（键盘 → onData → SSH stdin → 远端回显）的覆盖在 Rust live 测试
  `commands::terminal::tests::two_real_terminals_share_transport_and_close_independently`
  （`KADUOX_LIVE_TERMINAL_ALIAS=<别名> cargo test -- --ignored`），E2E 不合成键盘事件（WebView2 下不可靠）。
- 环境变量覆盖：`TAURI_DRIVER` / `EDGE_DRIVER` / `E2E_APP` 可指定驱动与被测二进制路径。
- `e2e/probe.js` 是手动诊断脚本（打印 URL/标题/页面源码/截图），不随套件运行。
