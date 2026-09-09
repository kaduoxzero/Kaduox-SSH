import React from 'react'

/**
 * 轻量 Markdown 渲染（AI 回答专用）：支持围栏代码块、行内代码、
 * 表格、标题、粗体/斜体、有序/无序列表、引用与段落。
 * 不引入第三方依赖，仅渲染受控子集，输出均为 React 节点（无 HTML 注入面）。
 */

function inline(text: string, keyPrefix: string): React.ReactNode[] {
  const nodes: React.ReactNode[] = []
  // 依次切分 `code`、**bold**、*italic*
  const pattern = /(`[^`\n]+`)|(\*\*[^*\n]+\*\*)|(\*[^*\n]+\*)/g
  let last = 0
  let match: RegExpExecArray | null
  let index = 0
  while ((match = pattern.exec(text)) !== null) {
    if (match.index > last) nodes.push(text.slice(last, match.index))
    const token = match[0]
    if (token.startsWith('`')) {
      nodes.push(<code key={`${keyPrefix}-c${index}`}>{token.slice(1, -1)}</code>)
    } else if (token.startsWith('**')) {
      nodes.push(<strong key={`${keyPrefix}-b${index}`}>{token.slice(2, -2)}</strong>)
    } else {
      nodes.push(<em key={`${keyPrefix}-i${index}`}>{token.slice(1, -1)}</em>)
    }
    last = match.index + token.length
    index += 1
  }
  if (last < text.length) nodes.push(text.slice(last))
  return nodes
}

function splitTableRow(line: string): string[] {
  const trimmed = line.trim().replace(/^\|/, '').replace(/\|$/, '')
  return trimmed.split('|').map((cell) => cell.trim())
}

function isTableDivider(line: string): boolean {
  return /^\|?[\s:|-]+\|?$/.test(line.trim()) && line.includes('-')
}

export function Markdown({ text }: { text: string }) {
  const blocks: React.ReactNode[] = []
  const lines = text.replace(/\r\n/g, '\n').split('\n')
  let i = 0
  let key = 0

  while (i < lines.length) {
    const line = lines[i]

    // 围栏代码块
    if (line.trimStart().startsWith('```')) {
      const lang = line.trim().slice(3).trim()
      const body: string[] = []
      i += 1
      while (i < lines.length && !lines[i].trimStart().startsWith('```')) {
        body.push(lines[i])
        i += 1
      }
      i += 1 // 跳过收尾 ```
      blocks.push(
        <pre className="md-code" key={key++} data-lang={lang || undefined}>
          <code>{body.join('\n')}</code>
        </pre>,
      )
      continue
    }

    // 表格：当前行含 | 且下一行是分隔行
    if (line.includes('|') && i + 1 < lines.length && isTableDivider(lines[i + 1])) {
      const header = splitTableRow(line)
      i += 2
      const rows: string[][] = []
      while (i < lines.length && lines[i].includes('|') && lines[i].trim() !== '') {
        rows.push(splitTableRow(lines[i]))
        i += 1
      }
      blocks.push(
        <table className="md-table" key={key++}>
          <thead>
            <tr>{header.map((cell, c) => <th key={c}>{inline(cell, `h${key}-${c}`)}</th>)}</tr>
          </thead>
          <tbody>
            {rows.map((row, r) => (
              <tr key={r}>{row.map((cell, c) => <td key={c}>{inline(cell, `r${key}-${r}-${c}`)}</td>)}</tr>
            ))}
          </tbody>
        </table>,
      )
      continue
    }

    // 标题
    const heading = /^(#{1,4})\s+(.*)$/.exec(line)
    if (heading) {
      const level = heading[1].length
      const content = inline(heading[2], `h${key}`)
      blocks.push(
        level === 1
          ? <h3 className="md-heading" key={key++}>{content}</h3>
          : level === 2
            ? <h4 className="md-heading" key={key++}>{content}</h4>
            : <h5 className="md-heading" key={key++}>{content}</h5>,
      )
      i += 1
      continue
    }

    // 引用
    if (line.trimStart().startsWith('>')) {
      const quote: string[] = []
      while (i < lines.length && lines[i].trimStart().startsWith('>')) {
        quote.push(lines[i].trimStart().replace(/^>\s?/, ''))
        i += 1
      }
      blocks.push(<blockquote className="md-quote" key={key++}>{inline(quote.join(' '), `q${key}`)}</blockquote>)
      continue
    }

    // 列表（连续 -/*/+ 或 1. 项）
    const isBullet = (l: string) => /^\s*[-*+]\s+/.test(l)
    const isOrdered = (l: string) => /^\s*\d+[.)]\s+/.test(l)
    if (isBullet(line) || isOrdered(line)) {
      const ordered = isOrdered(line)
      const items: string[] = []
      const test = ordered ? isOrdered : isBullet
      while (i < lines.length && test(lines[i])) {
        items.push(lines[i].replace(/^\s*(?:[-*+]|\d+[.)])\s+/, ''))
        i += 1
      }
      const children = items.map((item, n) => <li key={n}>{inline(item, `li${key}-${n}`)}</li>)
      blocks.push(
        ordered
          ? <ol className="md-list" key={key++}>{children}</ol>
          : <ul className="md-list" key={key++}>{children}</ul>,
      )
      continue
    }

    // 空行
    if (line.trim() === '') {
      i += 1
      continue
    }

    // 普通段落（合并连续行）
    const paragraph: string[] = [line]
    i += 1
    while (
      i < lines.length
      && lines[i].trim() !== ''
      && !lines[i].trimStart().startsWith('```')
      && !/^(#{1,4})\s/.test(lines[i])
      && !isBullet(lines[i])
      && !isOrdered(lines[i])
      && !lines[i].trimStart().startsWith('>')
      && !(lines[i].includes('|') && i + 1 < lines.length && isTableDivider(lines[i + 1]))
    ) {
      paragraph.push(lines[i])
      i += 1
    }
    blocks.push(<p className="md-paragraph" key={key++}>{inline(paragraph.join('\n'), `p${key}`)}</p>)
  }

  return <div className="md-body">{blocks}</div>
}
