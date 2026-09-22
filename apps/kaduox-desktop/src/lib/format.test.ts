import { describe, expect, it } from 'vitest'

import { errorMessage, fileName, formatBytes, formatDateTime, joinRemotePath, parentRemotePath } from './format'

describe('format helpers', () => {
  it('formats byte sizes with stable units', () => {
    expect(formatBytes(null)).toBe('—')
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(1024)).toBe('1.0 KB')
    expect(formatBytes(2 * 1024 * 1024)).toBe('2.0 MB')
  })

  it('formats timestamps with year, date and time', () => {
    expect(formatDateTime(null)).toBe('—')
    expect(formatDateTime(0)).toBe('—')
    const text = formatDateTime(Math.floor(Date.UTC(2025, 11, 19, 10, 14) / 1000))
    expect(text).toMatch(/^2025\D12\D\d{2}\D+\d{2}:\d{2}$/)
  })

  it('handles remote path navigation', () => {
    expect(joinRemotePath('/', 'etc')).toBe('/etc')
    expect(joinRemotePath('/var/', 'log')).toBe('/var/log')
    expect(parentRemotePath('/')).toBe('/')
    expect(parentRemotePath('/var/log/')).toBe('/var')
    expect(parentRemotePath('/home')).toBe('/')
  })

  it('extracts local file names and safe error messages', () => {
    expect(fileName('C:\\Users\\demo\\id_ed25519')).toBe('id_ed25519')
    expect(fileName('/tmp/archive.tar')).toBe('archive.tar')
    expect(errorMessage(new Error('connection refused'))).toBe('connection refused')
    expect(errorMessage('bad credentials')).toBe('bad credentials')
    expect(errorMessage({ reason: 'unknown' })).toBe('发生未知错误')
  })
})
