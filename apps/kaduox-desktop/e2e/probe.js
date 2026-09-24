import { remote } from 'webdriverio'
import path from 'node:path'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import fs from 'node:fs'

const here = path.dirname(fileURLToPath(import.meta.url))
const desktopRoot = path.resolve(here, '..')
const repoRoot = path.resolve(desktopRoot, '..', '..')

const tauriDriver = process.env.TAURI_DRIVER ?? path.join(repoRoot, 'tools', 'bin', 'tauri-driver.exe')
const edgeDriver = process.env.EDGE_DRIVER ?? path.join(repoRoot, 'tools', 'edgedriver', 'msedgedriver.exe')
const application = path.join(desktopRoot, 'src-tauri', 'target', 'debug', 'kaduox-ssh-desktop.exe')

const driver = spawn(tauriDriver, ['--native-driver', edgeDriver], { stdio: ['ignore', 'inherit', 'inherit'] })
await new Promise((r) => setTimeout(r, 3000))

const browser = await remote({
  hostname: '127.0.0.1',
  port: 4444,
  path: '/',
  logLevel: 'warn',
  capabilities: { 'tauri:options': { application } },
})

try {
  await new Promise((r) => setTimeout(r, 8000))
  console.log('URL:', await browser.getUrl())
  console.log('TITLE:', await browser.getTitle())
  const handles = await browser.getWindowHandles()
  console.log('WINDOWS:', handles)
  const source = await browser.getPageSource()
  fs.mkdirSync(path.join(here, 'artifacts'), { recursive: true })
  fs.writeFileSync(path.join(here, 'artifacts', 'probe-source.html'), source)
  console.log('SOURCE_LEN:', source.length)
  console.log('SOURCE_HEAD:', source.slice(0, 500))
  await browser.saveScreenshot(path.join(here, 'artifacts', 'probe.png'))
  console.log('screenshot saved')
} finally {
  await browser.deleteSession()
  driver.kill()
}
