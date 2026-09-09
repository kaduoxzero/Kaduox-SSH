import { beforeEach, describe, expect, it } from 'vitest'

import { loadAiProviders, loadAiSettings, loadTheme, removeAiProvider, saveAiProviders, saveAiSettings, saveTheme } from './storage'

describe('theme storage', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('uses the system preference when no setting exists', () => {
    expect(loadTheme()).toBe('dark')
  })

  it('persists only the versioned theme setting', () => {
    saveTheme('light')
    expect(loadTheme()).toBe('light')
    expect(localStorage.getItem('kaduox-desktop:settings:v1')).toBe(JSON.stringify({ theme: 'light' }))
  })

  it('ignores malformed or unsupported settings', () => {
    localStorage.setItem('kaduox-desktop:settings:v1', '{"theme":"sepia"}')
    expect(loadTheme()).toBe('dark')
    localStorage.setItem('kaduox-desktop:settings:v1', '{')
    expect(loadTheme()).toBe('dark')
  })
})

describe('AI settings storage', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('returns safe defaults and persists non-secret settings', () => {
    expect(loadAiSettings()).toMatchObject({
      mode: 'compatible',
      providerId: 'openai',
      providerName: 'OpenAI',
      endpoint: 'https://api.openai.com/v1/chat/completions',
      model: 'gpt-4o-mini',
    })

    saveAiSettings({
      mode: 'compatible',
      providerId: 'deepseek',
      providerName: 'DeepSeek',
      endpoint: 'http://127.0.0.1:11434/v1/chat/completions',
      model: 'qwen2.5-coder',
      permissionMode: 'full',
    })
    expect(loadAiSettings()).toEqual({
      mode: 'compatible',
      providerId: 'deepseek',
      providerName: 'DeepSeek',
      endpoint: 'http://127.0.0.1:11434/v1/chat/completions',
      model: 'qwen2.5-coder',
      permissionMode: 'full',
    })
    expect(localStorage.getItem('kaduox-desktop:ai-settings:v1')).not.toContain('apiKey')
    expect(localStorage.getItem('kaduox-desktop:ai-settings:v1')).not.toContain('rememberApiKey')
  })

  it('falls back when an AI setting is malformed', () => {
    localStorage.setItem('kaduox-desktop:ai-settings:v1', '{"mode":"unsafe"}')
    expect(loadAiSettings().mode).toBe('compatible')
  })

  it('keeps provider profiles separate from the API key', () => {
    expect(loadAiProviders().map((provider) => provider.name)).toEqual(['OpenAI', 'Ollama', 'LM Studio'])
    saveAiProviders([{ id: 'custom', name: 'DeepSeek', endpoint: 'https://api.deepseek.com/v1/chat/completions', model: 'deepseek-chat' }])
    expect(loadAiProviders()).toEqual([{ id: 'custom', name: 'DeepSeek', endpoint: 'https://api.deepseek.com/v1/chat/completions', model: 'deepseek-chat' }])
    expect(localStorage.getItem('kaduox-desktop:ai-providers:v1')).not.toContain('apiKey')
  })

  it('defaults to approval permission mode and removes provider profiles', () => {
    expect(loadAiSettings().permissionMode).toBe('approval')
    localStorage.setItem('kaduox-desktop:ai-settings:v1', '{"permissionMode":"full"}')
    expect(loadAiSettings().permissionMode).toBe('full')
    localStorage.setItem('kaduox-desktop:ai-settings:v1', '{"permissionMode":"unsafe"}')
    expect(loadAiSettings().permissionMode).toBe('approval')

    const providers = loadAiProviders()
    const next = removeAiProvider(providers, 'ollama')
    expect(next.map((provider) => provider.id)).toEqual(['openai', 'lm-studio'])
    expect(loadAiProviders().map((provider) => provider.id)).toEqual(['openai', 'lm-studio'])
  })
})
