# Kaduox-SSH 全面质量测试报告

- 日期：2026-09-24
- 版本：v0.33.0-rc.20（commit 0c376a4）
- 测试类型：自动化基线 + 变更回归 + 深度安全审计 + 双平台真机集成 + E2E/视觉回归（新增入库）
- 缺陷策略：只报告，不修

---

## 1. Overview（结论摘要）

自动化基线与真机功能全部通过；但深度审计发现 **1 个 S0（命令分类器换行符绕过）+ 4 个 S1**，均位于 AI 命令执行/会话管理路径。

**发布结论：FAIL（阻止发布）** —— S0 缺陷在默认 approval 模式下可被远端内容/AI 触发无审批执行任意命令，必须先修复 C-01，建议同批修复 C-02~C-04。

## 2. Scope

| 覆盖 | 内容 |
|------|------|
| Rust workspace | core / cli / daemon / hosts / mcp 全量单测 + clippy + fmt |
| desktop | Rust 57 测试 + 前端 vitest 48 + tsc + vite build |
| 变更回归 | 近 5 提交（分类器、exec stdin、AI 并发、SFTP 校验、GPU 采集） |
| 安全审计 | 路径穿越、命令分类器、AI 权限、并发生命周期、凭据、MCP/daemon、前端 XSS、编码 |
| 真机 | linux(123.57.239.56)、linx1(192.168.101.128)、Windows loopback(本地机器) |
| E2E | WebdriverIO + tauri-driver 7 条冒烟 + 6 页视觉基线（已入库 e2e/） |

**不覆盖**：性能压测、混沌注入、macOS 实机、forward_live_test（需跳板密码）、core Windows live 测试（需密钥认证材料）。

## 3. Environment

Windows 11 (10.0.26200) + WebView2 153.0.4234.48；Rust workspace debug profile；Node 24。真机：linux=Ubuntu 6.8 OpenSSH；linx1=LAN Linux；本地机器=Windows OpenSSH loopback（密码经系统 keyring）。

## 4. Strategy

风险驱动：P0 = AI 命令执行链、SFTP 路径安全、会话生命周期；P1 = 认证/跳板/转发/采集；P2 = UI 渲染与兼容性。新增 E2E 设施入库（apps/kaduox-desktop/e2e/，含 pixelmatch 视觉回归，终端 canvas 区域屏蔽比对）。

## 5. Execution

| 套件 | 结果 |
|------|------|
| cargo test --workspace | **286 过 0 失败**（2 ignored：Windows live，需密钥材料） |
| desktop cargo test | **57 过 0 失败**（3 ignored：live 测试） |
| 前端 vitest | **48 过 0 失败**；tsc 干净；vite build 干净 |
| clippy / fmt | 干净 |
| scripts pytest | 18 过 |
| 定向回归 | 分类器 17 过、resume 6 过 |
| 真机 | 见 §5.1 |
| E2E | **7/7 过**（连续 2 轮稳定），视觉基线 6 页建立并二次比对通过 |
| desktop live 终端测试（linx1） | 通过（双终端共享传输、独立关闭、远端无僵尸 shell） |

### 5.1 真机执行记录

| 场景 | linux | linx1 | Windows loopback |
|------|-------|-------|------------------|
| probe/认证（keyring 非交互） | ✓ | ✓ | ✓ |
| exec 单命令 + 中文输出 | ✓ | ✓ | ✓（单 token 限制见 D-01） |
| SFTP 上传/下载回环（UTF-8 内容） | ✓ hash 一致 | — | ✓ |
| 递归下载（symlink 跳过、无目录逃逸） | ✓ skipped=1 正确 | — | — |
| 断点续传（真前缀 4MiB → 续传 hash 一致） | ✓ | — | — |
| 跳板链（-J keyring 认证） | — | ✓（loopback 经跳板） | ✓ |
| GBK 终端输出 | — | — | ✓（桌面端正确渲染「版本/保留所有权利」） |
| 双终端共享传输 live 测试 | — | ✓ | 不适用（测试脚本为 POSIX 语法） |

## 6. Coverage

需求/规则/状态/风险覆盖见各 Phase；失败模式覆盖：symlink 逃逸、resume 边界、跳板认证、断线重连（E2E-07）。缺口：性能、混沌、macOS、五跳夹具（desktop-five-hop-fixture.sh 未跑）。

## 7. Defects（按严重度）

### S0（1）

| ID | 位置 | 标题 | 证据 |
|----|------|------|------|
| C-01 | command_classify.rs:301-351 + util.rs:22-30 | **换行符不拆分不拒绝，分类器完全绕过**：`ls\nrm -rf /` 首词 `ls` 命中只读白名单判 ReadOnly，approval 模式自动执行，远端逐行跑 `rm -rf /`；Windows 同理 `ver\r\ndel /s /q C:\` | split_segments 只拆 `;&\|`；validate_command 仅拒 NUL。修复建议：拒绝 `\n\r` 或换行按最高保守级分段 |

### S1（4）

| ID | 位置 | 标题 |
|----|------|------|
| C-02 | command_classify.rs:165-180 | 粘连重定向漏检：`echo x>/etc/cron.d/x`（token 不以 `>` 开头）判 ReadOnly → 无审批任意写文件 |
| C-03 | command_classify.rs:31-42 | 只读白名单含代码执行器：`python -c`/`node -e`/`find -exec`/`xargs`/`awk system()`；`curl -F`/`wget` 可外发数据 |
| C-04 | command_classify.rs:416-420 | PowerShell 写动词表漏 `Invoke-WebRequest/RestMethod/Command/WmiMethod` → 数据外泄判 ReadOnly |
| C-05 | state.rs:144-197 vs connection.rs:239-249 | heal 重连进行中用户断开 → reconnect 末尾无条件 insert，**僵尸会话复活**（disconnect 不取 reconnect_locks，无代际校验） |

### S2（10，摘要）

- C-06 terminal.rs:140-146：持锁跨 await，远端 hang 时 close_terminal/disconnect_host 永久挂死
- C-07 state.rs：重连无退避 → 主机宕机时每轮询触发完整重连锤击
- C-08 state.rs:177-180：reconnect 先删条目后连接，失败则条目永久丢失
- C-09 manager.rs:320-346：密码认证 NonReusable，并发同 alias 连接误报「绑定了不同认证源」
- C-10 forward.rs:147-149：accept 错误静默退出 → 转发假死（UI 显示活跃）
- C-11 state.rs：非主动断线时本地/动态转发监听器不回收，端口僵尸占用
- F-01 分类器外部：MCP `kssh_download` localPath 仅查 NUL，可写任意本地路径（需 KADUOX_MCP_ALLOW_MUTATIONS=1）
- F-02 桌面 `execute_command`（命令栏）无风险分类/审批（与终端等价，但 XSS 即放大；当前零 innerHTML 纪律是关键防线）
- SFTP-01 Windows 保留名/尾随点空格未处理（CON、foo. vs foo 归一化覆盖）
- SFTP-03 同目标并发传输写同一 staging，无去重/锁

### S3（18，摘要）

wmic 词级放行、手动 os_type 压探测的错配风险、AI tool 输出无防注入包裹（full 模式叠加效应）、审计缺批准批次 ID、resume 盲信本地前缀（语义注意）、TOCTOU 符号链接窗口、engine 路径失败清理缺失、read_remote_file 无流式上限、core 递归无深度限制、两套 entry 名校验不一致、daemon 连接数无上限、MCP 单行无限长、密钥内存未 zeroize、CSP 残留 dev 源、终端历史密码过滤仅靠前端、转发 bind 0.0.0.0 无确认、reconnect_locks 只增不减、指标采集无 per-alias 单飞。

### 真机新发现（Phase D）

| ID | 严重度 | 发现 |
|----|--------|------|
| D-01 | S2 | **CLI exec 对 Windows 目标不可用多词命令**：render_posix 用 POSIX 单引号渲染，经 cmd /c 包装后引号错乱（`cmd /c ver` 报 `'ver" 不是内部或外部命令`）。单 token（ver/whoami/hostname）正常。Linux 正常（bash -c 兼容单引号） |
| D-02 | S3 | CLI `-J` 不解析主机库别名（`-J linx1` 按 DNS 解析失败），须写 `user@host` |
| D-03 | S3 | 跳板认证失败且无 keyring 凭据时 CLI 阻塞等待交互式提示，无非 tty 超时（自动化场景挂起） |
| D-04 | 提示 | 桌面 live 终端测试脚本为 POSIX 语法，Windows 目标会超时失败——用例设计如此，非产品缺陷 |

### 已验证安全（审计通过项摘要）

历史修复全部完整无残留（路径穿越三件套、平台注入、busy 锁、Drop-close、GBK 边界）；凭据走 OS keyring；daemon IPC 有 UID/SID 校验不监听 TCP；协议帧有界；前端零 innerHTML/dangerouslySetInnerHTML；Tauri capabilities 最小化；AI endpoint 校验 + 错误不回显密钥；主机库 0600/原子写/反 symlink；并发单飞/租约计数/锁中毒处理正确。

## 8. Performance

未执行（范围外）。无基线 SLO。

## 9. Security

见 §7。核心攻击链：远端主机输出（或被投毒文件内容）→ AI 上下文 → 分类器 ReadOnly 误判（C-01/C-02/C-03/C-04）→ approval 模式自动执行。当前默认模式即受影响。

## 10. Risks（剩余风险）

| 剩余风险 | 影响 | 概率 | 缓解 | 接受？ |
|----------|------|------|------|--------|
| macOS 实机未验收 | 发布质量 | 中 | release 前补真机 | 待办 |
| 性能/压测未做 | 大文件/高并发未知 | 低 | 后续专项 | 接受 |
| forward live 测试未跑（缺跳板密码） | 跳板转发 | 低 | 已有 banner 用例待启用 | 接受 |
| E2E 仅 loopback 单主机 | 真实远距链路 UI | 低 | 后续加远程别名参数化 | 接受 |
| CLI exec Windows 多词命令（D-01） | CLI 用户 | 高 | 列入修复 | 不接受 |

## 11. Blockers（未测范围）

- core Windows live 测试（client.rs:1351/1440）：需密钥认证材料，当前 keyring 为密码
- forward_live_test：需 KADUOX_LIVE_JUMP_PASSWORD
- desktop-five-hop-fixture：需专用夹具环境

## 12. Regression

L1 变更回归全绿；E2E 视觉基线已建立，后续变更可自动回归（`npm run test:e2e`，基线更新 `$env:UPDATE_BASELINE='1'`）。

## 13. Conclusion

工程质量底盘扎实（391+ 自动化测试、真机验证通过、并发与凭据设计严谨），但 AI 命令分类器存在系统性绕过面（S0+3×S1 同源：词法分析不完整）。该子系统是「AI 自动执行」的信任根，当前状态不满足其安全承诺。

## 14. Release Recommendation

**FAIL** —— v0.33.0 正式版发布前必须：
1. 修复 C-01（S0）并补换行/粘连重定向回归测试
2. 修复 C-02/C-03/C-04（S1 同源，建议重写分类器词法层：引入迷你 shell 分词器而非 token 启发式）
3. 修复 C-05（僵尸会话）与 C-06（关闭挂死）
4. 修复 D-01（CLI Windows exec）或文档化限制
5. 修复后重跑 workspace + desktop + E2E 全套件

其余 S2/S3 可排期至后续迭代。

---

# 修复后附录（2026-09-25，rc.21）

32 项修复全部落地（批次 1-3 + 8 项补漏），4 项显式接受不修（见 §7 备注）。分类器按词法重构完成并新增 40+ 攻击串回归测试。

**修复后验证**：
- Rust workspace 195 + desktop 69 + 前端 48 + pytest 18 全绿；clippy/fmt/tsc 干净
- E2E 7/7 连续通过，视觉基线比对一致
- 真机复测：linux/linx1/Windows loopback 的 probe/exec/SFTP 回环/跳板链（`-J linx1`，keyring 免交互）全部正常；CLI Windows 多词命令（D-01）实测修复
- 攻击串重放：`ls\nrm -rf /` → Dangerous + 执行入口拒绝；`echo x>/etc/cron.d/x` → Modify；`python -c`/`find -exec`/`Invoke-RestMethod` → 非只读；`wmic process call create` → Modify

**结论更新：PASS（rc.21 可发布）**。剩余风险：macOS 实机验收、性能压测、forward live 测试（缺跳板密码）维持原计划跟踪。
