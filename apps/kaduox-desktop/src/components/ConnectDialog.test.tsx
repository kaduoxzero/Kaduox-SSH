import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi, beforeEach } from 'vitest'

import { ConnectDialog } from './ConnectDialog'
import type { AuthenticationRequest, Host, Session } from '../lib/types'

type OnConnect = (alias: string, authentication: AuthenticationRequest) => Promise<Session>

function mockConnect(): ReturnType<typeof vi.fn<OnConnect>> {
  return vi.fn<OnConnect>(async () => ({}) as Session)
}

const keyPassphraseStatus = vi.fn<(path: string) => Promise<boolean>>()
const deleteKeyPassphrase = vi.fn<(path: string) => Promise<boolean>>()

vi.mock('../lib/desktop', () => ({
  pickIdentityFile: vi.fn(async () => null),
  deleteStoredPassword: vi.fn(async () => true),
  keyPassphraseStatus: (path: string) => keyPassphraseStatus(path),
  deleteKeyPassphrase: (path: string) => deleteKeyPassphrase(path),
}))

const host: Host = {
  alias: 'Production API',
  address: '10.24.8.17',
  port: 22,
  user: 'deploy',
  identityFile: 'C:\\Users\\demo\\.ssh\\id_ed25519',
  groups: [],
  tags: [],
  note: null,
  hostKeyPolicy: 'strict',
  jumpChain: null,
  lastConnectedUnix: null,
  connectionCount: 0,
  lastAuthMethod: 'privateKey',
  hasStoredPassword: false,
}

function renderDialog(onConnect: ReturnType<typeof mockConnect> = mockConnect()) {
  return {
    onConnect,
    ...render(
      <ConnectDialog
        host={host}
        onClose={vi.fn()}
        onConnected={vi.fn()}
        onConnect={onConnect}
        onCredentialChanged={vi.fn(async () => {})}
      />,
    ),
  }
}

describe('ConnectDialog key passphrase saving', () => {
  beforeEach(() => {
    keyPassphraseStatus.mockReset().mockResolvedValue(false)
    deleteKeyPassphrase.mockReset().mockResolvedValue(true)
  })

  it('keeps the save-passphrase checkbox unchecked by default and sends savePassphrase only when opted in', async () => {
    const onConnect = mockConnect()
    renderDialog(onConnect)

    const passphraseInput = screen.getByLabelText(/密钥口令/)
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument()

    fireEvent.change(passphraseInput, { target: { value: 'key-secret' } })
    const checkbox = screen.getByRole('checkbox')
    expect(checkbox).not.toBeChecked()

    fireEvent.submit(screen.getByRole('button', { name: '连接' }).closest('form')!)
    await waitFor(() => expect(onConnect).toHaveBeenCalled())
    expect(onConnect.mock.calls[0][1]).toEqual({
      kind: 'privateKey',
      path: host.identityFile,
      passphrase: 'key-secret',
      savePassphrase: false,
    })
  })

  it('sends savePassphrase true when the user opts in', async () => {
    const onConnect = mockConnect()
    renderDialog(onConnect)

    fireEvent.change(screen.getByLabelText(/密钥口令/), { target: { value: 'key-secret' } })
    fireEvent.click(screen.getByRole('checkbox'))
    fireEvent.submit(screen.getByRole('button', { name: '连接' }).closest('form')!)
    await waitFor(() => expect(onConnect).toHaveBeenCalled())
    expect(onConnect.mock.calls[0][1]).toMatchObject({ savePassphrase: true, passphrase: 'key-secret' })
  })

  it('shows the stored-passphrase placeholder and remove button when a passphrase is saved', async () => {
    keyPassphraseStatus.mockResolvedValue(true)
    renderDialog()

    await waitFor(() =>
      expect(screen.getByLabelText(/密钥口令/)).toHaveAttribute('placeholder', '留空以使用已保存口令'),
    )
    const remove = screen.getByRole('button', { name: /删除已保存口令/ })
    fireEvent.click(remove)
    await waitFor(() => expect(deleteKeyPassphrase).toHaveBeenCalledWith(host.identityFile))
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: /删除已保存口令/ })).not.toBeInTheDocument(),
    )
  })
})
