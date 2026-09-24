import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { PNG } from 'pngjs'
import pixelmatch from 'pixelmatch'

const here = path.dirname(fileURLToPath(import.meta.url))
const baselineDir = path.resolve(here, '..', 'baseline')
const artifactsDir = path.resolve(here, '..', 'artifacts')

const DEFAULT_THRESHOLD = 0.005 // 差异像素占比 < 0.5% 通过

function ensureDirs() {
  fs.mkdirSync(baselineDir, { recursive: true })
  fs.mkdirSync(artifactsDir, { recursive: true })
}

/**
 * 对当前页面截图并与基线比对。
 * - UPDATE_BASELINE=1 时重建基线。
 * - maskSelectors 中的元素区域（如终端 canvas）在两张图中同时涂黑，不参与比对。
 */
export async function expectVisualMatch(name, { maskSelectors = [], threshold = DEFAULT_THRESHOLD } = {}) {
  ensureDirs()
  const shot = Buffer.from(await browser.takeScreenshot(), 'base64')
  const baselinePath = path.join(baselineDir, `${name}.png`)

  if (process.env.UPDATE_BASELINE === '1' || !fs.existsSync(baselinePath)) {
    fs.writeFileSync(baselinePath, shot)
    return { status: 'baseline-written', name }
  }

  const actual = PNG.sync.read(shot)
  const expected = PNG.sync.read(fs.readFileSync(baselinePath))
  if (actual.width !== expected.width || actual.height !== expected.height) {
    fs.writeFileSync(path.join(artifactsDir, `${name}.actual.png`), shot)
    throw new Error(
      `视觉回归 [${name}] 尺寸不一致: 实际 ${actual.width}x${actual.height}, 基线 ${expected.width}x${expected.height}。` +
      `如为预期变更请用 UPDATE_BASELINE=1 重建基线。`,
    )
  }

  const dpr = await browser.execute(() => window.devicePixelRatio || 1)
  for (const selector of maskSelectors) {
    const element = await $(selector)
    if (await element.isExisting()) {
      const location = await element.getLocation()
      const size = await element.getSize()
      const rect = {
        x: location.x * dpr,
        y: location.y * dpr,
        width: size.width * dpr,
        height: size.height * dpr,
      }
      blackout(actual, rect)
      blackout(expected, rect)
    }
  }

  const diff = new PNG({ width: actual.width, height: actual.height })
  const mismatched = pixelmatch(actual.data, expected.data, diff.data, actual.width, actual.height, {
    threshold: 0.15,
    includeAA: false,
  })
  const ratio = mismatched / (actual.width * actual.height)

  if (ratio >= threshold) {
    fs.writeFileSync(path.join(artifactsDir, `${name}.actual.png`), PNG.sync.write(actual))
    fs.writeFileSync(path.join(artifactsDir, `${name}.diff.png`), PNG.sync.write(diff))
    throw new Error(
      `视觉回归 [${name}] 差异 ${(ratio * 100).toFixed(2)}% 超过阈值 ${(threshold * 100).toFixed(1)}%（${mismatched} 像素）。` +
      `产物见 e2e/artifacts/，预期变更请用 UPDATE_BASELINE=1 重建基线。`,
    )
  }
  return { status: 'matched', name, ratio }
}

function blackout(png, rect) {
  const x0 = Math.max(0, Math.floor(rect.x))
  const y0 = Math.max(0, Math.floor(rect.y))
  const x1 = Math.min(png.width, Math.ceil(rect.x + rect.width))
  const y1 = Math.min(png.height, Math.ceil(rect.y + rect.height))
  for (let y = y0; y < y1; y += 1) {
    for (let x = x0; x < x1; x += 1) {
      const idx = (y * png.width + x) * 4
      png.data[idx] = 0
      png.data[idx + 1] = 0
      png.data[idx + 2] = 0
      png.data[idx + 3] = 255
    }
  }
}
