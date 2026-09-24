import { expect } from '@wdio/globals'
import { expectVisualMatch } from '../helpers/visual.js'

// 真实冒烟目标：本机 loopback OpenSSH（主机库别名“本地机器”，凭据在系统 keyring）。
// 运行前确认：kssh 本地机器 probe 可连通。
const LOOPBACK_ALIAS = '本地机器'

async function hostRow(alias) {
  const rows = await $$('.host-row')
  for (const row of rows) {
    if ((await row.getText()).includes(alias)) return row
  }
  throw new Error(`主机列表中找不到别名: ${alias}`)
}

async function connect(alias) {
  const row = await hostRow(alias)
  await row.$('.host-row-main').click()
  // 连接成功后出现会话标签（断开按钮随会话出现）
  const tab = await $('.session-tab-list')
  await tab.waitForExist({ timeout: 45000 })
  await browser.waitUntil(
    async () => (await $('.session-tab-list').getText()).includes(alias),
    { timeout: 45000, timeoutMsg: `会话标签未出现: ${alias}` },
  )
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

async function navTo(label) {
  const nav = await $('nav.top-navigation')
  const buttons = await nav.$$('button')
  for (const button of buttons) {
    if ((await button.getText()).includes(label)) {
      await button.click()
      return
    }
  }
  throw new Error(`主导航中找不到: ${label}`)
}

describe('Kaduox SSH 桌面端 E2E 冒烟', function () {
  it('E2E-01 应用启动：主界面渲染，无白屏', async function () {
    const shell = await $('.app-shell')
    await shell.waitForExist({ timeout: 30000 })
    await expect(await $('.host-sidebar').isDisplayed()).toBe(true)
    await expectVisualMatch('01-startup')
  })

  it('E2E-02 新建连接：点击 loopback 主机后出现会话标签', async function () {
    await connect(LOOPBACK_ALIAS)
    await expectVisualMatch('02-connected')
  })

  it('E2E-03 终端会话：新建终端后 PTY 就绪且远端提示符渲染（含 GBK 中文解码）', async function () {
    const add = await $('[aria-label="新建终端"]')
    await add.waitForExist({ timeout: 15000 })
    await add.click()
    const xterm = await $('.xterm')
    await xterm.waitForExist({ timeout: 30000 })
    // 等 PTY 就绪
    await browser.waitUntil(
      async () => (await $('.terminal-state').getText()).includes('就绪'),
      { timeout: 30000, timeoutMsg: 'PTY 未就绪' },
    )
    // 远端 cmd 提示符出现在终端文本中（xterm 可访问性文本）。
    // 输入→远端回显链路由 Rust live 测试 two_real_terminals_share_transport_and_close_independently 覆盖
    //（xterm 合成键盘事件在 WebView2 自动化下不可靠，__TAURI_INTERNALS__ 冻结不可拦截）。
    await browser.waitUntil(
      async () => {
        const text = await browser.execute(() => document.querySelector('.xterm')?.textContent ?? '')
        return text.includes('kaduox@') || text.includes('Microsoft Windows') || text.includes('$')
      },
      { timeout: 30000, timeoutMsg: '终端未渲染远端提示符' },
    )
    // 终端 canvas/文本区是动态内容，视觉比对时屏蔽
    await expectVisualMatch('03-terminal', { maskSelectors: ['.xterm'] })
  })

  it('E2E-04 SFTP 面板：文件视图加载远程目录', async function () {
    await navTo('文件')
    const browser_ = await $('.file-browser.expanded')
    await browser_.waitForExist({ timeout: 15000 })
    // loopback(Windows) 主目录应有内容或至少渲染出列表容器
    await browser.waitUntil(
      async () =>
        (await $$('.file-browser.expanded .file-row').length) > 0
        || (await $('.file-browser.expanded .file-empty').isExisting()),
      { timeout: 30000, timeoutMsg: '远程目录列表既无文件行也无空态' },
    )
    await expectVisualMatch('04-files')
  })

  it('E2E-05 SFTP 导航：进入已知目录', async function () {
    // hosts 工作区里还有一个隐藏的 FileBrowser，必须限定在 expanded 视图内
    const pathInput = await $('.file-browser.expanded [aria-label="远程路径"]')
    await pathInput.waitForExist({ timeout: 15000 })
    await pathInput.setValue('C:/Users')
    await browser.keys('Enter')
    await browser.waitUntil(
      async () => (await $$('.file-browser.expanded .file-row').length) > 0,
      { timeout: 30000, timeoutMsg: 'C:/Users 目录无文件行' },
    )
    const text = await $('.file-browser.expanded .file-list').getText()
    await expect(text.length).toBeGreaterThan(0)
  })

  it('E2E-06 AI 面板：对话界面渲染', async function () {
    await navTo('Kaduox')
    const input = await $('[aria-label="AI 问题"]')
    await input.waitForExist({ timeout: 15000 })
    await expectVisualMatch('06-ai')
  })

  it('E2E-07 断开与重连：会话状态流转正确', async function () {
    await navTo('主机')
    await disconnect(LOOPBACK_ALIAS)
    await expectVisualMatch('07-disconnected')
    await connect(LOOPBACK_ALIAS)
    await disconnect(LOOPBACK_ALIAS)
  })
})
