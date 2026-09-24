import { spawn } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const desktopRoot = path.resolve(here, '..')
const repoRoot = path.resolve(desktopRoot, '..', '..')

const tauriDriver = process.env.TAURI_DRIVER
  ?? path.join(repoRoot, 'tools', 'bin', 'tauri-driver.exe')
const edgeDriver = process.env.EDGE_DRIVER
  ?? path.join(repoRoot, 'tools', 'edgedriver', 'msedgedriver.exe')
const application = process.env.E2E_APP
  ?? path.join(desktopRoot, 'src-tauri', 'target', 'debug', 'kaduox-ssh-desktop.exe')

let driver

export const config = {
  runner: 'local',
  specs: [path.join(here, 'specs', '**', '*.spec.js')],
  maxInstances: 1,
  hostname: '127.0.0.1',
  port: 4444,
  path: '/',
  logLevel: 'warn',
  framework: 'mocha',
  reporters: ['spec'],
  mochaOpts: { timeout: 120000 },
  waitforTimeout: 30000,
  connectionRetryTimeout: 60000,
  connectionRetryCount: 3,
  capabilities: [{
    'tauri:options': { application },
    'ms:edgeOptions': { args: [] },
  }],

  async onPrepare() {
    driver = spawn(tauriDriver, ['--native-driver', edgeDriver], {
      stdio: ['ignore', 'inherit', 'inherit'],
    })
    // tauri-driver 启动监听需要一点时间
    await new Promise((resolve) => setTimeout(resolve, 3000))
    if (driver.exitCode !== null) {
      throw new Error(`tauri-driver exited early with code ${driver.exitCode}`)
    }
  },

  onComplete() {
    if (driver && driver.exitCode === null) driver.kill()
  },
}
