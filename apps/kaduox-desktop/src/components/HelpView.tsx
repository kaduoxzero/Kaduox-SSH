import { useState } from 'react'
import { openHelpLink } from '../lib/desktop'
import { errorMessage } from '../lib/format'

export function HelpView() {
  const [error, setError] = useState('')
  const open = (page: 'project' | 'manual' | 'releases') => { void openHelpLink(page).catch((cause) => setError(errorMessage(cause))) }
  return <main className="help-view">
    <header><h1>Kaduox SSH 使用说明</h1><p>保存机器，点击打开终端；需要经过中转时，在最终目标上配置路径。</p>
      <div className="help-links"><button className="primary-button" onClick={() => open('project')}>GitHub 项目</button><button className="secondary-button" onClick={() => open('manual')}>完整使用说明</button><button className="secondary-button" onClick={() => open('releases')}>下载与版本发行</button></div>
      {error && <p role="alert" className="inline-error">{error}</p>}
    </header>
    <section><h2>主机、终端与文件夹</h2><p>添加主机时填写地址、用户和凭据。密码保存在系统凭据库，不写入主机配置。点击主机直接打开终端：未连接则自动认证，已连接则复用 SSH。每个终端有独立的目录、变量和输出；“＋”只新增终端，“×”只关闭该终端。断开主机会关闭它的全部终端和转发。</p><p>目标主机与专用中转分区显示；只有你选为“专用中转”的机器进入下区。两者兼用放在目标区，也可加入跳板链。左栏新建文件夹，点击文件夹可折叠或重命名，在编辑主机中选择文件夹完成移动。文件夹不对应服务器目录。</p></section>
    <section><h2>SSH 跳转：先选最终目标，再选中间机器</h2><p>分别保存各台机器及其凭据。编辑最终目标，创建跳板链，按入口到出口加入中转机器；最多 5 台中转，加上最终目标。每段默认 SSH 22 端口，每台使用自己的账号和凭据。</p><pre>本机 → 中转 A（demo@192.0.2.10）→ 目标 B（deploy@198.51.100.20）</pre><p>点击目标 B 即按整条路线连接。不要将目标 B 再加到跳板列表，也不需要创建端口转发。修改路线只在下次连接生效。</p></section>
    <section><h2>端口转发：搬运连接，不会自动运行网站</h2><dl><dt>本地 -L</dt><dd>本机监听 → SSH 隧道 → 远端可访问的服务。用于从本机访问远端数据库或内网页面。</dd><dt>远程 -R</dt><dd>远端监听 → SSH 隧道 → 本机侧可访问的服务。用于让公网机器代理本机已有服务。</dd><dt>SOCKS -D</dt><dd>本机提供 SOCKS5 代理，需要在浏览器或应用里配置代理地址。</dd></dl><p>远程转发默认绑定 127.0.0.1，只能在服务器自己访问。公网访问还需要服务器允许转发和相应 GatewayPorts 设置、对外监听、防火墙及安全组放行。本软件不会自动修改这些配置。</p><p>这不是 Nginx：没有自动 HTTPS、域名路由或负载均衡。SSH 只加密隧道段，公开的服务仍需自己的 HTTPS 和鉴权。不要将数据库、管理接口或无密码服务直接暴露。断开 SSH 后转发失效。</p></section>
    <section><h2>主机密钥策略：确认“对面是谁”</h2><dl><dt>严格</dt><dd>只接受已记录的可信服务器指纹。首次使用需通过可信渠道核验并添加指纹。</dd><dt>首次信任</dt><dd>第一次自动记住指纹，以后指纹变化拒绝连接；第一次仍应核对服务器身份。</dd><dt>不安全</dt><dd>不核验服务器身份，仅用于隔离测试。它不是“免密码”，也不会提高权限。</dd></dl><p>指纹变化可能是重装，也可能是冒充。先联系管理员核实，不要直接切换“不安全”。</p></section>
    <section><h2>信息、文件、记录和 Kaduox AI</h2><p>信息页打开后自动采集当前展示机器的 CPU、GPU、内存、磁盘和网络，每 10 秒刷新。SFTP 与终端共享连接。运行记录每页 50 条，记录显式执行的命令；不会记录终端按键或密码提示。</p><p>Kaduox AI 调用你配置的兼容厂商接口，可获取模型列表或手填模型。API 密钥保存在系统凭据库；发送主机上下文需要勾选授权。AI 不会自动执行回答中的命令。不要向 AI 粘贴密码、私钥或令牌。</p></section>
    <section><h2>MCP 接入 Agent</h2><p>下载 kaduox-ssh-mcp.exe，使用 stdio 接入。默认只读；命令执行及文件传输需要另外显式开启。示例路径应改为你的实际安装位置。</p><pre>{'{\n  "mcpServers": {\n    "kaduox": { "command": "C:/Tools/Kaduox/kaduox-ssh-mcp.exe" }\n  }\n}'}</pre><p>读写权限和全部参数请查看完整使用说明；不要把密码写入 Agent 配置。</p></section>
    <footer>示例使用保留文档地址，不是真实服务器。项目：https://github.com/kaduoxzero/Kaduox-SSH</footer>
  </main>
}
