import { describe, expect, it } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'

import { Markdown } from './markdown'

describe('Markdown renderer', () => {
  it('renders tables with header and rows', () => {
    const html = renderToStaticMarkup(
      <Markdown text={'| 容器名 | 镜像 |\n|--------|------|\n| **api** | `nginx:1` |\n| db | pg |'} />,
    )
    expect(html).toContain('<table')
    expect(html).toContain('<th>容器名</th>')
    expect(html).toContain('<strong>api</strong>')
    expect(html).toContain('<code>nginx:1</code>')
  })

  it('renders fenced code blocks and headings', () => {
    const html = renderToStaticMarkup(
      <Markdown text={'### 运行中\n```sh\ndocker ps\n```'} />,
    )
    expect(html).toContain('md-heading')
    expect(html).toContain('md-code')
    expect(html).toContain('docker ps')
  })

  it('renders lists and inline styles without raw HTML injection', () => {
    const html = renderToStaticMarkup(
      <Markdown text={'- **bold** and *ital*\n1. first\n2. second\n<script>alert(1)</script>'} />,
    )
    expect(html).toContain('<strong>bold</strong>')
    expect(html).toContain('<em>ital</em>')
    expect(html).toContain('<ol')
    // 原始 HTML 永远作为文本转义输出
    expect(html).not.toContain('<script>')
  })
})
