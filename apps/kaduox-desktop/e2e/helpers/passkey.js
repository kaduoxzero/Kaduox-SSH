// E2E-08 密钥口令冒烟的环境准备：已授权密钥的加密副本、主机库条目、
// Windows 凭据库残留口令清理。
//
// 目标端点刻意用 <用户>@localhost:22 而不是 127.0.0.1：两者都到本机 sshd，
// 但凭据库账户按 user@host:port 计算，localhost 没有已存密码，
// 前端才不会把 e2e-passkey 误判为「密码直连」而跳过连接对话框。
//
// 关于密钥来源：Windows OpenSSH 对管理员账户只读
// __PROGRAMDATA__/ssh/administrators_authorized_keys（写它需要提权，自动化做不了），
// 用户级 ~/.ssh/authorized_keys 不生效。因此这里把已授权的默认私钥
// 复制为带测试口令的加密副本（ssh-keygen -p 原地重加密，公钥不变即已授权）。
// 副本落在 gitignore 的 e2e/artifacts/ 下且带口令加密，cleanup() 删除；
// 其保护级别不低于同账户下本就未加密的原文件。
import { execFileSync, spawnSync } from 'node:child_process'
import fs from 'node:fs'
import net from 'node:net'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))

export const PASSKEY_ALIAS = 'e2e-passkey'
export const PASSKEY_PASSPHRASE = 'e2e-passkey-fixture-passphrase'

const keyDir = path.join(here, '..', 'artifacts', 'passkey')
// 主机库与 UI 统一使用正斜杠路径；core 归一化时自行转换。
const keyPath = path.join(keyDir, 'e2e_ed25519').replaceAll('\\', '/')
const sourceKey = path.join(os.homedir(), '.ssh', 'id_ed25519')

/** 本机 sshd 不可达时返回 false，调用方据此跳过整组规格。 */
export function loopbackReachable() {
  return new Promise((resolve) => {
    const socket = net.connect(22, '127.0.0.1')
    socket.setTimeout(5000)
    socket.once('connect', () => {
      socket.destroy()
      resolve(true)
    })
    socket.once('error', () => resolve(false))
    socket.once('timeout', () => {
      socket.destroy()
      resolve(false)
    })
  })
}

/** 把已授权的无口令默认密钥复制并重加密为测试口令的副本（幂等）。 */
export function ensureKeyPair() {
  const target = path.join(keyDir, 'e2e_ed25519')
  if (fs.existsSync(target)) return
  // OpenSSH 私钥 base64 前缀含 cipher/kdf 双 "none" 即未加密
  const header = fs.readFileSync(sourceKey, 'utf8')
  if (!header.includes('b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ')) {
    throw new Error(`${sourceKey} 不是无口令密钥，无法制作测试副本`)
  }
  fs.mkdirSync(keyDir, { recursive: true })
  fs.copyFileSync(sourceKey, target)
  fs.copyFileSync(`${sourceKey}.pub`, `${target}.pub`)
  // 副本继承仓库目录 ACL，OpenSSH 工具链要求私钥仅属主可读；收紧后再重加密
  if (process.platform === 'win32') {
    const user = `${os.userInfo().username}`
    execFileSync('icacls', [target, '/inheritance:r', '/grant:r', `${user}:F`], { stdio: 'pipe' })
  } else {
    fs.chmodSync(target, 0o600)
  }
  // 原地重加密：旧口令空 → 测试口令
  execFileSync('ssh-keygen', ['-p', '-q', '-P', '', '-N', PASSKEY_PASSPHRASE, '-f', target], { stdio: 'pipe' })
}

/** 公钥授权复用既有 administrators_authorized_keys，无需部署/撤回。 */
export function deployPubkey() {}
export function withdrawPubkey() {}

function hostsTomlPath() {
  return path.join(process.env.APPDATA ?? path.join(os.homedir(), 'AppData', 'Roaming'), 'Kaduox-SSH', 'hosts.toml')
}

/** 以「删除同名旧块 + 追加新块」保证幂等；不触碰其它主机。 */
export function ensureHostEntry() {
  const file = hostsTomlPath()
  removeHostEntry()
  const entry = [
    '',
    '[[host]]',
    `alias = "${PASSKEY_ALIAS}"`,
    'address = "localhost"',
    'port = 22',
    `user = "${os.userInfo().username}"`,
    `identity_file = "${keyPath}"`,
    'groups = []',
    'tags = []',
    'host_key_policy = "accept-new"',
    '',
  ].join('\n')
  fs.appendFileSync(file, entry)
}

export function removeHostEntry() {
  const file = hostsTomlPath()
  if (!fs.existsSync(file)) return
  const content = fs.readFileSync(file, 'utf8')
  // 删除 alias 匹配的整个 [[host]] 块（到下一个 [[host]] 或文件结尾），\r\n/\n 兼容
  const pattern = new RegExp(
    `\\r?\\n?\\[\\[host\\]\\]\\r?\\nalias = "${PASSKEY_ALIAS}"[\\s\\S]*?(?=\\r?\\n?\\[\\[host\\]\\]|$)`,
  )
  fs.writeFileSync(file, content.replace(pattern, ''))
}

/**
 * keyring windows-native 的凭据 target 形如 `<account>.kssh`；
 * 口令账户名为 `key:<小写反斜杠绝对路径>`（与 core normalize_key_path 对齐）。
 * cmdkey 删除失败视为无残留，静默忽略。
 */
export function clearStoredPassphrase() {
  if (process.platform !== 'win32') return
  const normalized = path.resolve(keyDir, 'e2e_ed25519').toLowerCase()
  spawnSync('cmdkey', [`/delete:key:${normalized}.kssh`], { stdio: 'ignore' })
}

export function cleanup() {
  withdrawPubkey()
  removeHostEntry()
  clearStoredPassphrase()
  // 删除已授权密钥的加密副本
  fs.rmSync(keyDir, { recursive: true, force: true })
}
