import Bot from 'lucide-react/dist/esm/icons/bot'
import FileKey2 from 'lucide-react/dist/esm/icons/file-key-2'
import KeyRound from 'lucide-react/dist/esm/icons/key-round'
import LockKeyhole from 'lucide-react/dist/esm/icons/lock-keyhole'
import ShieldCheck from 'lucide-react/dist/esm/icons/shield-check'
import Trash2 from 'lucide-react/dist/esm/icons/trash-2'
import X from 'lucide-react/dist/esm/icons/x'
import { useState } from 'react'

import { deleteStoredPassword, pickIdentityFile } from '../lib/desktop'
import { errorMessage } from '../lib/format'
import type { AuthenticationRequest, AuthKind, Host, Session } from '../lib/types'

interface ConnectDialogProps {
  host: Host
  onClose: () => void
  onConnected: (session: Session) => void
  onConnect: (alias: string, authentication: AuthenticationRequest) => Promise<Session>
  onCredentialChanged: () => Promise<void>
}

const methods: Array<{ id: AuthKind; label: string; icon: typeof Bot }> = [
  { id: 'auto', label: '自动', icon: Bot },
  { id: 'privateKey', label: '私钥', icon: FileKey2 },
  { id: 'agent', label: 'Agent', icon: KeyRound },
  { id: 'password', label: '密码', icon: LockKeyhole },
]

export function ConnectDialog({
  host,
  onClose,
  onConnected,
  onConnect,
  onCredentialChanged,
}: ConnectDialogProps) {
  const initialMethod: AuthKind = host.identityFile ? 'privateKey' : host.hasStoredPassword ? 'password' : 'auto'
  const [method, setMethod] = useState<AuthKind>(initialMethod)
  const [keyPath, setKeyPath] = useState(host.identityFile ?? '')
  const [passphrase, setPassphrase] = useState('')
  const [password, setPassword] = useState('')
  const [savePassword, setSavePassword] = useState(true)
  const [busy, setBusy] = useState(false)
  const [dialogError, setDialogError] = useState<string | null>(null)

  const browse = async () => {
    const selected = await pickIdentityFile()
    if (selected) setKeyPath(selected)
  }

  const connect = async (event: React.FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setDialogError(null)
    let authentication: AuthenticationRequest
    if (method === 'privateKey') {
      authentication = { kind: 'privateKey', path: keyPath || null, passphrase: passphrase || null }
    } else if (method === 'password') {
      authentication = { kind: 'password', password: password || null, savePassword }
    } else if (method === 'agent') {
      authentication = { kind: 'agent' }
    } else if (method === 'keyboardInteractive') {
      authentication = { kind: 'keyboardInteractive', secret: password }
    } else {
      authentication = { kind: 'auto', passphrase: passphrase || null }
    }
    try {
      const session = await onConnect(host.alias, authentication)
      onConnected(session)
    } catch (error) {
      setDialogError(errorMessage(error))
      setBusy(false)
    }
  }

  const removeCredential = async () => {
    setBusy(true)
    setDialogError(null)
    try {
      await deleteStoredPassword(host.alias)
      await onCredentialChanged()
      setPassword('')
    } catch (error) {
      setDialogError(errorMessage(error))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section className="connect-dialog" role="dialog" aria-modal="true" aria-labelledby="connect-title">
        <div className="dialog-heading">
          <div className="dialog-host-icon"><span>&gt;_</span></div>
          <div>
            <span className="eyebrow">SECURE CONNECTION</span>
            <h2 id="connect-title">连接到 {host.alias}</h2>
            <p>{host.user}@{host.address}:{host.port}</p>
          </div>
          <button className="icon-button dialog-close" type="button" onClick={onClose} aria-label="关闭连接窗口">
            <X size={18} />
          </button>
        </div>

        <form onSubmit={connect}>
          <div className="auth-methods" role="tablist" aria-label="认证方式">
            {methods.map(({ id, label, icon: Icon }) => (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={method === id}
                className={method === id ? 'active' : ''}
                onClick={() => setMethod(id)}
              >
                <Icon size={16} aria-hidden="true" />
                {label}
              </button>
            ))}
          </div>

          <div className="auth-fields">
            {method === 'privateKey' && (
              <>
                <label className="field full-width">
                  <span>私钥文件</span>
                  <div className="input-action">
                    <FileKey2 size={16} />
                    <input value={keyPath} onChange={(event) => setKeyPath(event.target.value)} placeholder="选择 Ed25519 / ECDSA 私钥" />
                    <button type="button" onClick={browse}>浏览</button>
                  </div>
                </label>
                <label className="field full-width">
                  <span>密钥口令（可选）</span>
                  <input type="password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} autoComplete="off" />
                </label>
              </>
            )}
            {method === 'auto' && (
              <>
                <div className="auth-explanation">
                  依次尝试 Windows OpenSSH Agent、主机配置的密钥，以及默认 Ed25519 / ECDSA 密钥。
                </div>
                <label className="field full-width">
                  <span>密钥口令（可选）</span>
                  <input type="password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} autoComplete="off" />
                </label>
              </>
            )}
            {method === 'agent' && (
              <div className="auth-explanation">
                使用 Windows OpenSSH Agent 或 Pageant 中已加载的密钥；RSA 密钥也必须通过此路径使用。
              </div>
            )}
            {method === 'password' && (
              <>
                <label className="field full-width">
                  <span>登录密码</span>
                  <input
                    type="password"
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                    placeholder={host.hasStoredPassword ? '留空以使用已保存密码' : '输入本次连接密码'}
                    autoComplete="off"
                  />
                </label>
                <label className="check-field">
                  <input type="checkbox" checked={savePassword} onChange={(event) => setSavePassword(event.target.checked)} />
                  <span>连接成功后保存到 Windows 凭据管理器</span>
                </label>
                {host.hasStoredPassword && (
                  <button className="credential-remove" type="button" onClick={removeCredential} disabled={busy}>
                    <Trash2 size={14} /> 删除已保存密码
                  </button>
                )}
              </>
            )}
          </div>

          <div className="trust-summary">
            <ShieldCheck size={17} />
            <span>主机密钥策略：<strong>{host.hostKeyPolicy === 'strict' ? '严格验证' : host.hostKeyPolicy === 'accept-new' ? '首次信任，变更拒绝' : '不安全模式'}</strong></span>
          </div>
          {dialogError && <div className="inline-error" role="alert">{dialogError}</div>}
          <div className="dialog-actions">
            <button className="secondary-button" type="button" onClick={onClose} disabled={busy}>取消</button>
            <button className="primary-button" type="submit" disabled={busy}>
              {busy ? '正在建立安全连接…' : '连接'}
            </button>
          </div>
        </form>
      </section>
    </div>
  )
}
