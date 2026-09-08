import { describe, expect, it } from 'vitest'

import { errorMessage, fileName, formatBytes, joinRemotePath, parentRemotePath } from './format'

describe('format helpers', () => {
  it('formats byte sizes with stable units', () => {
    expect(formatBytes(null)).toBe('—')
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(1024)).toBe('1.0 KB')
    expect(formatBytes(2 * 1024 * 1024)).toBe('2.0 MB')
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
