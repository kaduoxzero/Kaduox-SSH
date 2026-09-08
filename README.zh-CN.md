# Kaduox-SSH

[English](README.md) | **简体中文**

<img src="apps/kaduox-desktop/public/app-icon.png" width="72" height="72" alt="Kaduox SSH 图标">

Kaduox-SSH 是一款桌面 SSH 客户端与 Rust 工具集，提供终端、远程文件、SSH 跳板链、系统数据屏、端口转发和 Agent 接口。Windows 客户端支持深浅主题，不需要另行部署 Web 服务。

[下载 v0.33.0-rc.1](https://github.com/kaduoxzero/Kaduox-SSH/releases/tag/v0.33.0-rc.1) · [完整桌面手册](docs/DESKTOP_GUIDE.zh-CN.md) · [MCP 接入 Agent](docs/MCP.md) · [版本说明](docs/releases/v0.33.0-rc.1.md)

## 下载与安装

本次预发布仅提供 **Windows x64** 程序，不包含 Linux/macOS 桌面包。产物为本地手动构建，**未进行 Authenticode 签名**，Windows 可能提示发行者未知。请从本仓库 Releases 下载，并核对 SHA-256 校验值。

| 下载 | 适用场景 |
| --- | --- |
| [Windows 安装包](https://github.com/kaduoxzero/Kaduox-SSH/releases/download/v0.33.0-rc.1/Kaduox-SSH-0.33.0-rc.1-windows-x64-setup.exe) | 推荐普通用户使用，可选择安装位置和中文/英文安装界面，包含 WebView2 离线安装组件。 |
| [免安装桌面 EXE](https://github.com/kaduoxzero/Kaduox-SSH/releases/download/v0.33.0-rc.1/Kaduox-SSH-0.33.0-rc.1-windows-x64.exe) | 电脑已安装 WebView2 Runtime 时直接运行；配置仍保存在当前用户的应用数据目录，不随 EXE 移动。 |
| [CLI / TUI / Agent 工具包](https://github.com/kaduoxzero/Kaduox-SSH/releases/download/v0.33.0-rc.1/Kaduox-SSH-0.33.0-rc.1-windows-x64-tools.zip) | 包含命令行、TUI、daemon、fleet、inventory 和 MCP 共六个程序。 |
| [MCP 接口 EXE](https://github.com/kaduoxzero/Kaduox-SSH/releases/download/v0.33.0-rc.1/kaduox-ssh-mcp.exe) | 只需要接入 Agent，不安装桌面客户端。 |
| [SHA256SUMS.txt](https://github.com/kaduoxzero/Kaduox-SSH/releases/download/v0.33.0-rc.1/SHA256SUMS.txt) | 核对下载文件，可使用 PowerShell：`Get-FileHash -Algorithm SHA256 <文件>`。 |

发行包不包含个人服务器、密码、API 密钥或用户数据文件。新用户配置为空；升级不会自动抹除已有用户保存的数据。若旧 CLI/TUI/MCP 与桌面端共用主机库，请一起升级。

## 桌面版快速上手

1. **保存主机**：填写显示名称、地址、SSH 端口和登录用户。名称支持中文、空格和括号。密码保存到 Windows 凭据管理器，也可使用私钥或 SSH Agent。
2. **打开终端**：点击已保存主机即可使用保存的认证信息连接。已连接时再次点击主机，或点击终端栏 **＋**，会新增一个独立终端，不是新建主机。仅在凭据缺失或认证失败等情况下补充输入。
3. **区分目标和中转**：选择“目标机器”“专用中转”或“两者兼用”。专用中转在侧栏单独展示。自行创建、折叠、重命名逻辑文件夹，在主机编辑页的下拉框中移动主机。
4. **使用工作区**：SFTP 浏览、上传和下载；信息页展示 CPU、内存、磁盘、网络及可用 GPU 指标，打开立即采集，之后每 **10 秒**刷新当前对象。运行记录保存在本地，每页 **50 条**，可滚动翻页。关闭客户端不保留终端进程状态。

Windows 使用 **Ctrl K** 搜索主机名、地址或标签，Enter 打开对应终端。顶部“帮助”可查看使用说明和 GitHub 项目入口。

### SSH 跳板：在最终目标上配置

```text
本机 → 中转 A (demo@192.0.2.10:22) → 目标 B (deploy@198.51.100.20:22)
```

以上仅为文档示例地址。先保存 A、B 及各自的认证信息，将 A 设为专用中转或兼用；再编辑 **B**，在 SSH 连接路径中把 A 加入中转列表。之后直接点击 B，即自动经 A 连接 B，不需要手工输入第二条 ssh。

最多支持 **5 台中转 + 1 台最终目标**，按连接顺序排列。每一段都是 SSH，分别使用对应机器的用户、端口和凭据；修改路径后需重连。普通 SSH 跳转不需要额外创建端口转发规则。

### 端口转发与主机密钥

| 类型 | 监听端 → 服务端 |
| --- | --- |
| 本地 `-L` | 本机监听 → SSH 隧道 → 远端主机可访问的服务 |
| 远程 `-R` | 远端监听 → SSH 隧道 → 本机可访问的服务 |
| SOCKS5 `-D` | 本机 SOCKS5 监听 → SSH 隧道 → 远端网络 |

端口转发传递 TCP 流量，**不会启动网站，也不是 Nginx 的替代品**，不提供 HTTP 路由、TLS 终止或负载均衡。默认仅监听回环地址。若需通过远程转发对公网开放，还需设置监听地址、服务器转发/GatewayPorts 权限和防火墙；服务自身需要 HTTPS 与鉴权。详见[端口转发说明](docs/DESKTOP_GUIDE.zh-CN.md#端口转发不是-nginx)。

**主机密钥**用于核验服务器身份，不是登录密码：“严格”要求已有信任记录；“首次信任”首次记录指纹、之后变化则拒绝；“不安全”跳过普通身份验证，仅适合隔离测试。匹配的吊销记录仍会拒绝密钥。详见[主机密钥策略](docs/DESKTOP_GUIDE.zh-CN.md#主机密钥策略)。

### Kaduox AI 与 MCP

**Kaduox AI 运维助手**仅调用用户配置的 OpenAI-compatible 聊天接口，可自定义厂商名称、地址、API 密钥，获取模型列表或手动填写模型名，**不提供离线回答模式**。远程接口使用 HTTPS，本机回环接口可用 HTTP。密钥保存在系统凭据库；只有用户勾选后才发送主机上下文，AI 建议不会自动执行。

外部 Agent 下载 `kaduox-ssh-mcp.exe` 后，以绝对路径配置 **stdio** MCP：

```json
{
  "mcpServers": {
    "kaduox": {
      "command": "C:/Tools/Kaduox/kaduox-ssh-mcp.exe",
      "env": {
        "KADUOX_MCP_ALLOW_EXEC": "0",
        "KADUOX_MCP_ALLOW_MUTATIONS": "0"
      }
    }
  }
}
```

将示例路径替换为实际下载位置。程序由 Agent 启动，无需 Web 服务或手动打开终端窗口。默认只提供主机元数据、路由、基础信息、系统指标和 SFTP 列表查询；命令执行及文件传输需要显式授权。MCP 复用自己的连接，不共享桌面终端标签。完整工具清单和开关见 [MCP 文档](docs/MCP.md)。

## 使用与开发文档

- [完整桌面手册](docs/DESKTOP_GUIDE.zh-CN.md)：安装、凭据、文件夹、跳板、信息采集、AI 和本地数据位置。
- [MCP / Agent 接入](docs/MCP.md)、[TUI](docs/TUI.md)、[会话工作区](docs/SESSION_WORKSPACE.md)、[Fleet](docs/FLEET_EXEC.md)、[Inventory](docs/INVENTORY.md)。
- [OpenSSH 兼容性](docs/OPENSSH_CONFIG.md)、[主机证书](docs/HOST_CERTIFICATES.md)、[安全策略](SECURITY.md)、[发布策略](docs/RELEASE.md)。

共享 Rust 核心关注受控内存、明确的信任校验、已认证连接复用与长期可维护性。下面描述核心和 CLI 的工程契约，不代表每一项都已有桌面操作入口。

## 当前能力

- SSH 远程连接、命令执行与交互式 PTY Shell
- OpenSSH `~/.ssh/config` Host 解析、受控 `Include` 展开（含单次 `${ENV}`、字面 `%%` 与原生 `%l`/`%L` 本机主机名路径处理）、受限 `Match all` / `Match originalhost` 计算，并对未支持结构保持 fail-closed
- 跨平台 OpenSSH 用户配置可信校验：Unix 使用 uid/mode，Windows 使用 owner/DACL，并且都绑定到后续实际读取的同一个已打开文件对象
- 基于当前目标匹配的明文或 OpenSSH hashed `known_hosts` `@cert-authority` 进行 OpenSSH Host Certificate 验证
- 对普通主机密钥、证书 subject key 和证书签发 CA 执行明文及 OpenSSH hashed `@revoked` 吊销检查
- ProxyJump 链与 ProxyCommand 传输
- 密码、keyboard-interactive、Ed25519/ECDSA 私钥、OpenSSH Agent、Pageant/Windows Agent 认证
- 本地 RSA 私钥签名因安全策略禁用，但仍支持通过外部 SSH Agent 使用 RSA 认证
- 在受影响的 RSA 验证 feature 被禁用期间，提前明确拒绝仅提供 RSA 主机密钥的服务器
- 可选 Agent Forwarding
- 本地转发（`-L`）、远程转发（`-R`）和 SOCKS5 动态转发（`-D`）
- 在已有 SSH 传输上通过 `sudo` 快速切换远端操作系统用户
- strict / accept-new / 显式 insecure 主机密钥验证策略
- 有界内存 SFTP 单文件/递归上传下载、`.kaduox.part` 断点续传、默认原子暂存、进度与协作式取消
- 使用普通用户 SFTP 暂存 + sudo 的特权单文件及递归目录上传
- 先计划后变更的本地到远端目录同步，并对破坏性删除要求显式 `--delete`
- 递归传输/同步提供显式符号链接策略：`skip` 与 fail-closed 的 `reject`；不会跟随链接
- 进程内已认证连接管理器和显式长生命周期 lease
- `kssh-tui` 多主机 Session Dashboard、OpenSSH Host picker、远端文件浏览、Shell/sudo Shell、文件操作和可恢复传输任务
- `kssh-fleet` 有界并发的多目标 typed command 执行，逐主机故障隔离与输出上限
- `kssh-inventory` 离线 Inventory 校验、列表和嵌套 group 展开
- 私有持久化主机库：主机别名、按最近使用排序、命名 jump chain（`kssh hosts`、`kssh chains`）
- `kssh-daemon` 单用户本地 IPC 连接复用 daemon；`kssh` 优先通过它复用已认证传输，失败时回退直连；逐跳交互式 jump 认证通过私有 IPC 通道桥接
- 可选的登录密码持久化，直接写入操作系统凭据存储（Windows 凭据管理器、macOS 钥匙串、Linux Secret Service），通过 `kssh credentials set/check/delete` 管理；Kaduox-SSH 不会把密码写入自有文件
- 通过 `kssh completions <shell>` 生成 bash/zsh/fish/powershell/elvish 补全脚本
- Windows 桌面客户端：主机文件夹、独立终端、SFTP、跳板链、转发、运行历史、深浅主题、系统指标数据屏和外部兼容 API AI 助手（无离线模式）
- 本地 stdio MCP Server：默认只读的主机、路由、基础信息和 SFTP 查询，可按环境变量显式开放命令执行与文件传输，详见 [`docs/MCP.md`](docs/MCP.md)
- 已配置 Linux/macOS/Windows CI、MSRV、Clippy、依赖审计、release-policy 和真实 OpenSSH workflow 门禁；实际验证范围和编译限制见下文
- 确定性的四套发行包流程，包含每个二进制的 manifest 和最终 SHA-256 校验文件
- 稳定版 workflow 包含 Windows Authenticode、macOS Developer ID 签名/公证策略（本次手动桌面预发布不包含签名）
- 稳定版 workflow 包含四目标 SPDX 2.3 SBOM、provenance 与 archive-to-SBOM attestation 策略

SSH 登录用户在认证完成后无法被 SSH 协议本身修改。Kaduox-SSH 会在现有传输上继续打开额外 channel；交互式权限切换使用 `sudo -iu <user>`，远端命令切换用户使用 `sudo -n -u <user> -- sh -lc ...`。

特权上传不会假设 SFTP 可以修改 uid。Kaduox-SSH 会先以当前 SSH 登录用户暂存单文件或整棵目录树，再以指定远端操作系统用户执行显式 sudo 安装阶段，最后清理暂存数据。递归特权上传在目标目录已存在时会使用同级工作目录，并通过“删除旧目标后 rename”完成替换，因此不会把已有目录替换宣称为原子操作。

## 程序组成

| 程序 | 用途 |
| --- | --- |
| `kaduox-ssh-desktop` | Windows 图形客户端，以顶部链接中的桌面 EXE 和安装包发行。 |
| `kssh` | 单目标 shell、exec、传输、同步、转发、检查与诊断。 |
| `kssh-tui` | 多主机终端 Dashboard，使用显式连接 lease，支持有界命令广播。 |
| `kssh-daemon` | 单用户本地 IPC 服务，复用已认证的连接。 |
| `kssh-fleet` | 对显式目标或 Inventory group 执行有界并发命令，逐主机隔离故障。 |
| `kssh-inventory` | 不建立网络连接的 Inventory 校验、列表及嵌套分组展开。 |
| `kaduox-ssh-mcp` | 面向 Agent 的本地 stdio MCP 接口，默认只读。 |

这些前端不维护第二套 SSH 栈；认证、主机密钥/证书策略、ProxyJump/ProxyCommand、channel、SFTP、权限切换和资源限制都来自 `kaduox-ssh-core`。

## RSA 安全策略

当 Russh 的 RSA 依赖路径仍受 `RUSTSEC-2023-0071`（Marvin timing attack）影响时，Kaduox-SSH 会主动关闭 Russh 可选的本地 RSA signer。因此受影响的 `rsa` crate 不会进入最终应用的解析锁文件。

- Ed25519 和 ECDSA 私钥文件可以直接使用。
- 本地 RSA 私钥文件会 fail-closed，并给出明确的替代方案提示。
- RSA 用户认证仍可通过外部 SSH Agent 使用，因为私钥运算发生在 Agent 内部，而不是 Kaduox-SSH 进程中。
- 当前会在密钥交换阶段拒绝仅提供 RSA 主机密钥的服务器。服务器应提供 Ed25519 或 ECDSA 主机密钥。
- v0.16 不广告 RSA Host Certificate variant；真实证书 fixture 使用 Ed25519 CA + Ed25519 主机证书。Russh `rsa` feature 仍关闭时，不宣称 RSA CA/证书签名兼容。

在该安全公告获得可接受的上游修复并且完整安全测试实际执行通过之前，不应重新启用 Russh 的 `rsa` feature。

## OpenSSH 配置与主机信任兼容性

Kaduox-SSH 会从 `~/.ssh/config` 解析受支持的 `HostName`、`User`、`Port`、`IdentityFile`、`UserKnownHostsFile`、`ProxyCommand`、`ProxyJump` 等配置。

v0.14 新增受控 `Include` 展开，包括全局/`Host` 作用域、一行多路径、引号/转义、绝对路径、相对 `~/.ssh`、当前用户 `~/...`、`*`/`?`、lexical 顺序、嵌套 Include、隐藏文件规则，以及每个 included 文件结束后恢复父作用域。Host catalog 使用同一 Include 图。

v0.19 新增刻意受限的 `Match` 子集：支持独立的 `Match all`，以及单条件 `Match originalhost <pattern-list>`。`originalhost` 使用 HostName 改写前的查询 alias；pattern-list 不区分 ASCII 大小写、以逗号分隔、支持 `*`/`?`，并支持前导 `!` 否定。其它 Match 条件和组合继续 fail-closed。

v0.28 把这个 Match 子集正式整合进 Include scope machine。活跃 Host/Match 下的 Include 正常计算，返回 included 文件后恢复父 active state；inactive Host/Match 下的 Include 仍会实际打开文件、执行 trust/循环/资源限制/语法校验，但 included 文件中的 Host/Match 不能重新激活当前目标配置。这修复了旧路径中 child `Host <target>` 可能逃逸 inactive parent scope 的问题。

v0.29 新增 OpenSSH 风格的 Include `${NAME}` 环境变量展开。展开只执行一遍：环境变量值本身不会再次作为 `${...}` 或 `%` token 被扫描。变量缺失、空变量名、未闭合 `${...}`、非 UTF-8 环境值都会 fail-closed；展开后的路径继续受现有 16 KiB 单路径上限以及全部 Include trust/循环/资源边界约束。

v0.30 新增 OpenSSH 中完全不依赖连接状态的 `%%` escape：Include 源文本中的 `%%` 会折叠为一个字面 `%`。尤其不会用当前配置目录去近似 `%d`，因为 OpenSSH 的 `%d` 来自本地 passwd 记录的 `pw_dir`，而 Kaduox-SSH 当前配置 home 来自平台环境变量，两者可能不同。

v0.31 新增 OpenSSH Include `%l` / `%L` 本机主机名展开。`%l` 使用平台原生 `gethostname` 返回的完整主机名；`%L` 使用同一个 Include 参数内的主机名快照并在第一个 `.` 处截断。Unix 直接调用 `gethostname`；Windows 在首次需要该能力时按进程 once 初始化 Winsock 2.2，再调用 Winsock `gethostname`，其生命周期与 Rust std 的 Windows socket 初始化策略一致，不会在每次 Include 之间反复 startup/cleanup。主机名值本身不做进程级缓存，因此不同 Include 参数仍可重新查询。主机名查询失败或非 UTF-8 会 fail-closed，`${ENV}` 产生的 `%l`/`%L` 仍保持字面内容而不会被二次扫描。

v0.15 把根配置和所有嵌套 Include 统一放到同一配置文件信任边界。Unix 上要求实际打开文件为 regular file、owner 为当前进程 real uid 或 root、group/other 不可写；metadata 校验和解析读取使用同一个已打开 fd，避免独立 stat/read 的路径 TOCTOU。v0.26 把同一原则扩展到 Windows：owner/DACL 从已经打开的文件 HANDLE 获取，非受信主体的写权限 Allow ACE 会被拒绝，其他主体的只读访问仍允许，复杂 allow ACE 则 fail-closed 而不是做不完整近似。

v0.16 新增 fail-closed Host Certificate / `@cert-authority` / `@revoked` 语义：

- 证书算法默认不广告；只有最终目标或某个 ProxyJump hop 自己的 `UserKnownHostsFile` 中存在匹配的 `@cert-authority` 时才为该目标开启；
- marker host pattern 可使用明文或 OpenSSH `|1|base64-salt|base64-hmac-sha1` hashed name；
- Host Certificate 必须是 Host 类型；
- CA 必须属于当前目标匹配的受信 `@cert-authority`；
- 校验证书签名和当前有效期；
- principal 为空时不额外限制 hostname；存在 principal 时至少一个必须匹配目标 hostname，可使用 `*` / `?`；
- 当前不解释任何 certificate critical option，因此只要存在 critical option 就拒绝；
- 证书 subject key 或签发 CA 任一命中当前目标的 `@revoked` 都拒绝；
- 普通主机密钥也会在 known-hosts / explicit insecure 接受之前检查 `@revoked`；
- 证书验证失败不会降级成 embedded ordinary public key 再尝试接受。

仍然采用 fail-closed 的部分：

- 已支持 `Match all` 和单条件 `Match originalhost <pattern-list>`；`canonical`、`final`、`exec`、`localnetwork`、`host`、`tagged`、`command`、`user`、`localuser`、`version`、组合条件、条件否定/`criterion=value`、带引号或反斜杠转义的 Match 参数继续明确拒绝；
- Include 的 `%%`、`%l`、`%L` 已支持；其余 named percent token（`%C`、`%d`、`%h`、`%k`、`%n`、`%p`、`%r`、`%u`、`%i`、`%j`）、`~other-user` 和完整 bracket/collation glob 暂不实现，会明确报错而不是做近似处理；
- Include 最大 16 层、256 个处理文件、4 MiB 配置预算、单行 64 个路径、环境/percent 展开后单路径 16 KiB、单 wildcard component 1024 bytes，并检测循环 include；
- hashed marker name 使用 effective known-hosts target bytes 做精确 HMAC-SHA1 匹配，不执行大小写折叠或 wildcard；非默认端口使用 OpenSSH `[host]:port` 形式；
- RSA Host Certificate 与 RSA CA/证书签名兼容性当前不宣称支持；
- Windows SIDHistory 账户名等价兼容性有意比 Win32-OpenSSH 更严格：不同 SID 始终按不同主体处理，不会因反向解析得到相同账户名而扩展信任；
- 上游解析器只以布尔值暴露 `StrictHostKeyChecking`，需要精确行为时请使用 `--host-key strict`、`--host-key accept-new` 或 `--host-key insecure`。

完整契约见 `docs/OPENSSH_CONFIG.md` 和 `docs/HOST_CERTIFICATES.md`。

## 架构

```text
apps/
  kaduox-desktop/    # React 界面 + Tauri Windows 客户端（独立 Cargo workspace）
crates/
  kaduox-ssh-core/    # transport/auth/session/forward/SFTP/sync/privilege/manager 核心
  kaduox-ssh-hosts/   # 原子私有 TOML 主机库、别名与命名 jump chain
  kaduox-ssh-daemon/  # 单用户 IPC 连接复用 daemon（peer 身份校验、有界协议）
  kaduox-ssh-mcp/     # 面向 Agent 的本地 stdio MCP Server
  kaduox-ssh-cli/     # kssh + kssh-tui + kssh-fleet + kssh-inventory
```

核心层与具体前端解耦，TUI/Fleet/Inventory 复用相同的安全和传输实现。

## 构建

根工作区声明 Rust 1.85，但当前 Windows Pageant 依赖使用 let-chains，实际需要 **Rust 1.88 或更新版本**；桌面 manifest 同样声明 1.88。本次 Windows 预发布使用 **Rust 1.98.0** 构建，不宣称 Windows 1.85 兼容。Windows 编译需要当前稳定版 MSVC 工具链、Visual Studio C++ Build Tools / Windows SDK 和 NASM。

在仓库根目录构建 CLI/TUI/daemon/fleet/inventory/MCP 六个程序：

```bash
cargo build --release --locked
```

产物位于 `target/release/`，Windows 文件带 `.exe` 后缀。桌面端单独构建，还需安装 Node.js/npm：

```powershell
cd apps/kaduox-desktop
npm ci
npm test
npm run tauri -- build --bundles nsis --ci -- --locked
```

使用 Tauri 构建命令，确保发行版内嵌前端页面。桌面 EXE 位于 `apps/kaduox-desktop/src-tauri/target/release/`，安装包位于其 `bundle/nsis/` 目录；打包过程中可能下载 WebView2 离线组件。在同一目录运行 `npm run tauri -- dev` 可启动原生客户端开发模式。

## 使用示例

```bash
# 交互式 Shell
kssh server.example.com --user deploy shell

# 在同一 SSH 传输上进入 root Shell
kssh server.example.com --user deploy shell --as-user root

# typed command
kssh server.example.com --user deploy exec -- uname -a

# Ed25519/ECDSA 私钥 / 密码
kssh server.example.com --user deploy --identity ~/.ssh/id_ed25519 shell
kssh server.example.com --user deploy --password shell

# RSA 仍可通过 ssh-agent 使用；直接 RSA 私钥文件会被拒绝
ssh-add ~/.ssh/id_rsa
kssh server.example.com --user deploy shell

# ProxyJump 与端口转发
kssh target.internal -J bastion.example -L 8080:127.0.0.1:80 tunnel
kssh target.internal -D 1080 tunnel

# 单文件和递归传输；reject 会在扫描到符号链接时 fail-closed
kssh server.example.com upload ./app.tar.zst /tmp/app.tar.zst
kssh server.example.com download /srv/logs ./logs -r --jobs 8 --symlinks reject

# 断点恢复
kssh server.example.com upload ./large.img /srv/large.img --resume

# 同步计划 / 应用
kssh server.example.com sync ./dist /srv/www/dist --dry-run
kssh server.example.com sync ./dist /srv/www/dist
kssh server.example.com sync ./dist /srv/www/dist --delete
kssh server.example.com sync ./dist /srv/www/dist --symlinks reject

# TUI
kssh-tui production

# Fleet
kssh-fleet -H web-01 -H web-02 --jobs 2 -- uname -a

# Inventory 离线校验
kssh-inventory check
```

普通主机密钥默认采用 `accept-new`：未知普通 key 会写入标准 OpenSSH `known_hosts`，变化的 key 会被拒绝。`--host-key strict` 要求普通 key 预先存在，或者服务端 Host Certificate 可由当前目标匹配的 CA 完整验证。`--host-key insecure` 只适合一次性/测试环境；当前目标匹配的 `@revoked` 仍优先拒绝。

## 文件传输与同步设计

文件传输使用有界缓冲区，大文件请求流水线交给 `russh-sftp`；Kaduox-SSH 负责稳定断点文件、原子最终替换、目录并发、进度、取消和特权暂存。文件并发最多 128、SFTP pipelined write 最多 128、packet size 4 KiB–4 MiB，并且估算的 `file_concurrency × write_concurrency × packet_size` 总写窗口不得超过 512 MiB。

同步会先扫描本地和远端目录树并构建 typed action plan。默认保留仅存在于远端的内容；只有显式 `--delete` 才允许删除和类型冲突替换。`--dry-run` 永远不修改远端目录树。

递归上传/下载和同步已经提供显式符号链接策略。`skip` 是兼容默认值，会忽略链接/重解析点且绝不跟随；`reject` 会先执行 fail-closed preflight，只要扫描到 link-like entry 就中止。Follow/preserve 仍不支持，因为它们需要有界的循环/越界处理，以及完整的跨平台 target encoding 语义。

## 验证与发布

CI 已配置 Ubuntu、macOS、Windows、Rust 1.85 MSRV；Quality 配置 Clippy `-D warnings`、依赖审计和 release-policy；Linux OpenSSH workflow 会启动真实 `sshd` fixture 覆盖认证、跳板机、Agent、Host Certificate/吊销、SFTP、同步、权限切换、转发、typed command、diagnostics 和 fleet。

v0.16 的独立 Host Certificate fixture 会实际用 `ssh-keygen` 创建 Ed25519 CA/Host Certificate，并验证：匹配 CA 成功、principal mismatch 拒绝、签发 CA `@revoked` 拒绝、普通主机 key 即使 explicit insecure 也不能绕过 `@revoked`。

**v0.33.0-rc.1 Windows 桌面预发布**从 `develop` 手动本地构建，不代表跨平台 Actions 发布门禁通过。本次检查通过前端 35 项、主机库 17 项、桌面后端 26 项测试，以及相关 Clippy 和前端生产构建；三项依赖真实远端凭据的桌面集成测试在本次检查中显式忽略。更早的真实跳板/终端验证记录单独保留在[桌面工作流文档](docs/DESKTOP_WORKFLOWS.md)。

本次 `cargo-audit 0.22.0` 检查两个 Cargo.lock 均未报告已知漏洞，但仍有撤回依赖版本、非 Windows 图形依赖维护/健全性提示。干净 Windows 安装环境和所有第三方 AI 厂商尚未穷举验证，范围与限制见[版本说明](docs/releases/v0.33.0-rc.1.md)。

独立的稳定版 workflow 要求 Windows/macOS 原生签名、四目标 SPDX 文件和 attestation matrix；发行资格检查涵盖主要产物校验值、SBOM 身份、签名、二进制版本及真实 Linux OpenSSH 路径。本次未签名、仅 Windows 的预发布不宣称满足这些稳定版门禁，也不晋升 `main`。

## 分支模型

```text
feat/* / fix/* / perf/* / ci/* -> develop -> release/* -> main
```

`develop` 是小版本开发集成分支，`main` 只用于稳定版/大版本发布晋升。Candidate 分支用于在不绕过门禁的情况下组装较大的预发布开发线。

## 尚未完成的安全敏感能力

- 超出当前 `all` / `originalhost` 子集的完整 OpenSSH `Match` 计算，以及剩余 named Include percent token/`~user`/完整 glob；
- RSA 安全依赖恢复后的 RSA Host Certificate/CA 兼容；
- Kaduox-SSH 自管理的加密凭据文件与自定义密钥管理模型（常见持久化需求已由操作系统凭据存储覆盖）；
- 符号链接 follow/preserve 传输/同步语义，以及有界循环、越界和跨平台 target 处理。

更多设计约束、验证门禁和发布策略见 `docs/OPENSSH_CONFIG.md`、`docs/HOST_CERTIFICATES.md`、`docs/RELEASE.md`、`docs/ARCHITECTURE.md`、`docs/TUI.md`、`docs/FLEET_EXEC.md` 和 `SECURITY.md`。

## 许可证

[MIT](LICENSE)。
