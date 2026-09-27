import { expect } from '@wdio/globals'
import {
  PASSKEY_ALIAS,
  PASSKEY_PASSPHRASE,
  clearStoredPassphrase,
  cleanup,
  deployPubkey,
  ensureHostEntry,
  ensureKeyPair,
  loopbackReachable,
} from '../helpers/passkey.js'

// E2E-08 密钥口令保存冒烟（真实链路：loopback sshd + 加密 Ed25519 + 系统凭据库）。
// 前置由 helpers/passkey.js 幂等准备：加密密钥生成、公钥部署、主机库条目、
// 凭据库残留清理。本机 sshd 不可达时整组跳过（与 CI 无 sshd 环境兼容）。
//
// 流程对应用户故事：加密密钥首次连接输一次口令并勾选保存 → 之后连接不再问 →
// 凭据库口令删除后恢复 fail-closed（必须重新输入）。

async function hostRow(alias) {
  const rows = await $$('.host-row')
  for (const row of rows) {
    if ((await row.getText()).includes(alias)) return row
  }
  throw new Error(`主机列表中找不到别名: ${alias}`)
}

async function clickHost(alias) {
  const row = await hostRow(alias)
  await row.$('.host-row-main').click()
}

async function waitSessionTab(alias) {
  const tab = await $('.session-tab-list')
  await tab.waitForExist({ timeout: 60000 })
  await browser.waitUntil(
    async () => (await $('.session-tab-list').getText()).includes(alias),
    { timeout: 60000, timeoutMsg: `会话标签未出现: ${alias}` },
  )
}

async function waitConnectDialog() {
  const dialog = await $('.connect-dialog')
  await dialog.waitForExist({ timeout: 60000 })
  return dialog
}

async function disconnect(alias) {
  const close = await $(`[aria-label="断开 ${alias}"]`)
  await close.waitForExist({ timeout: 15000 })
  await close.click()
  await browser.waitUntil(
    async () => !(await $(`[aria-label="断开 ${alias}"]`).isExisting()),
    { timeout: 15000, timeoutMsg: `会话标签未消失: ${alias}` },
  )
}

let skipSuite = false

describe('E2E-08 密钥口令保存（loopback 加密 Ed25519）', function () {
  before(async function () {
    ensureKeyPair()
    if (!(await loopbackReachable())) {
      console.warn('本机 sshd 不可达，跳过 E2E-08')
      skipSuite = true
      return
    }
    deployPubkey()
    clearStoredPassphrase()
    ensureHostEntry()
    // 主机条目是在应用启动后写入的，刷新 WebView 让前端重新拉取主机列表
    await browser.refresh()
    await browser.waitUntil(
      async () => {
        for (const row of await $$('.host-row')) {
          if ((await row.getText()).includes(PASSKEY_ALIAS)) return true
        }
        return false
      },
      { timeout: 30000, timeoutMsg: `主机列表刷新后仍无 ${PASSKEY_ALIAS}` },
    )
  })

  after(function () {
    if (skipSuite) return
    cleanup()
  })

  beforeEach(function () {
    if (skipSuite) this.skip()
  })

  it('08a 首连：auto 失败后弹对话框，输口令不勾选保存也能连上', async function () {
    await clickHost(PASSKEY_ALIAS)
    // auto 认证对加密密钥无口令可用 → 失败 → 连接对话框出现
    const dialog = await waitConnectDialog()
    // 私钥模式默认选中（主机配了 identityFile），口令框无「已保存」placeholder
    const passphraseInput = await dialog.$('input[type="password"]')
    await passphraseInput.waitForExist({ timeout: 10000 })
    await expect(await passphraseInput.getAttribute('placeholder')).not.toBe('留空以使用已保存口令')
    await expect(await dialog.$('.credential-remove').isExisting()).toBe(false)
    // 复选框仅在输入口令后出现，且默认不勾选
    await expect(await dialog.$('.check-field input[type="checkbox"]').isExisting()).toBe(false)
    await passphraseInput.setValue(PASSKEY_PASSPHRASE)
    const checkbox = await dialog.$('.check-field input[type="checkbox"]')
    await checkbox.waitForExist({ timeout: 5000 })
    await expect(await checkbox.isSelected()).toBe(false)
    await dialog.$('.primary-button').click()
    await waitSessionTab(PASSKEY_ALIAS)
    await disconnect(PASSKEY_ALIAS)
  })

  it('08b 勾选保存：连接成功后口令写入系统凭据库', async function () {
    await clickHost(PASSKEY_ALIAS)
    const dialog = await waitConnectDialog()
    const passphraseInput = await dialog.$('input[type="password"]')
    await passphraseInput.setValue(PASSKEY_PASSPHRASE)
    const checkbox = await dialog.$('.check-field input[type="checkbox"]')
    await checkbox.waitForExist({ timeout: 5000 })
    await checkbox.click()
    await expect(await checkbox.isSelected()).toBe(true)
    await dialog.$('.primary-button').click()
    await waitSessionTab(PASSKEY_ALIAS)
    await disconnect(PASSKEY_ALIAS)
  })

  it('08c 核心验收：再次连接不再弹对话框，凭据库口令自动生效', async function () {
    await clickHost(PASSKEY_ALIAS)
    // auto 直接用凭据库口令完成认证：不出现对话框，直接出现会话标签
    await waitSessionTab(PASSKEY_ALIAS)
    await expect(await $('.connect-dialog').isExisting()).toBe(false)
    await disconnect(PASSKEY_ALIAS)
  })

  it('08d 删除凭据库口令后 fail-closed：空口令连接报错', async function () {
    clearStoredPassphrase()
    await clickHost(PASSKEY_ALIAS)
    // auto 失败 → 对话框；私钥模式空口令提交 → 连接失败并展示错误
    const dialog = await waitConnectDialog()
    const passphraseInput = await dialog.$('input[type="password"]')
    await passphraseInput.waitForExist({ timeout: 10000 })
    await dialog.$('.primary-button').click()
    const error = await dialog.$('.inline-error')
    await error.waitForExist({ timeout: 60000 })
    await expect((await error.getText()).length).toBeGreaterThan(0)
    await dialog.$('[aria-label="关闭连接窗口"]').click()
    await browser.waitUntil(
      async () => !(await $('.connect-dialog').isExisting()),
      { timeout: 10000, timeoutMsg: '连接对话框未关闭' },
    )
  })
})
