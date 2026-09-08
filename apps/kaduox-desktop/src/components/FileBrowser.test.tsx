import { describe, expect, it } from 'vitest'

import { remoteHomePath } from '../lib/format'

describe('remote home path', () => {
  it('uses the real root home instead of /home/root', () => {
    expect(remoteHomePath('root')).toBe('/root')
  })

  it('uses the conventional home path for regular users', () => {
    expect(remoteHomePath('kaduox')).toBe('/home/kaduox')
    expect(remoteHomePath('  deploy  ')).toBe('/home/deploy')
  })
})
