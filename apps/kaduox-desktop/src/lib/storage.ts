import type { AiProviderProfile, AiSettings, Theme } from './types'

const SETTINGS_KEY = 'kaduox-desktop:settings:v1'
const AI_SETTINGS_KEY = 'kaduox-desktop:ai-settings:v1'
const AI_PROVIDERS_KEY = 'kaduox-desktop:ai-providers:v1'

interface PersistedSettings {
  theme: Theme
}

const defaultAiSettings: AiSettings = {
  mode: 'compatible',
  providerId: 'openai',
  providerName: 'OpenAI',
  endpoint: 'https://api.openai.com/v1/chat/completions',
  model: 'gpt-4o-mini',
}

export const defaultAiProviders: AiProviderProfile[] = [
  {
    id: 'openai',
    name: 'OpenAI',
    endpoint: 'https://api.openai.com/v1/chat/completions',
    model: 'gpt-4o-mini',
  },
  {
    id: 'ollama',
    name: 'Ollama',
    endpoint: 'http://127.0.0.1:11434/v1/chat/completions',
    model: 'qwen2.5-coder',
  },
  {
    id: 'lm-studio',
    name: 'LM Studio',
    endpoint: 'http://127.0.0.1:1234/v1/chat/completions',
    model: 'local-model',
  },
]

function isTheme(value: unknown): value is Theme {
  return value === 'light' || value === 'dark'
}

export function loadTheme(): Theme {
  try {
    const value = localStorage.getItem(SETTINGS_KEY)
    if (value) {
      const parsed = JSON.parse(value) as Partial<PersistedSettings>
      if (isTheme(parsed.theme)) return parsed.theme
    }
  } catch {
    // A disabled or full localStorage should never prevent application startup.
  }
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

export function saveTheme(theme: Theme): void {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify({ theme } satisfies PersistedSettings))
  } catch {
    // Theme persistence is best-effort; the active in-memory theme still applies.
  }
}

export function loadAiSettings(): AiSettings {
  try {
    const value = localStorage.getItem(AI_SETTINGS_KEY)
    if (value) {
      const parsed = JSON.parse(value) as Partial<AiSettings>
      return {
        mode: 'compatible',
        providerId: typeof parsed.providerId === 'string' && parsed.providerId.trim() ? parsed.providerId : defaultAiSettings.providerId,
        providerName: typeof parsed.providerName === 'string' && parsed.providerName.trim() ? parsed.providerName : defaultAiSettings.providerName,
        endpoint: typeof parsed.endpoint === 'string' && parsed.endpoint.trim() ? parsed.endpoint : defaultAiSettings.endpoint,
        model: typeof parsed.model === 'string' && parsed.model.trim() ? parsed.model : defaultAiSettings.model,
      }
    }
  } catch {
    // AI preferences are best-effort; an unavailable storage must not block startup.
  }
  return { ...defaultAiSettings }
}

export function saveAiSettings(settings: AiSettings): void {
  try {
    localStorage.setItem(AI_SETTINGS_KEY, JSON.stringify({
      mode: settings.mode,
      providerId: settings.providerId,
      providerName: settings.providerName,
      endpoint: settings.endpoint,
      model: settings.model,
    }))
  } catch {
    // Keep the active settings in memory when persistence is unavailable.
  }
}

function isProviderProfile(value: unknown): value is AiProviderProfile {
  if (!value || typeof value !== 'object') return false
  const profile = value as Partial<AiProviderProfile>
  return [profile.id, profile.name, profile.endpoint, profile.model]
    .every((item) => typeof item === 'string' && item.trim().length > 0)
}

export function loadAiProviders(): AiProviderProfile[] {
  try {
    const value = localStorage.getItem(AI_PROVIDERS_KEY)
    if (value) {
      const parsed: unknown = JSON.parse(value)
      if (Array.isArray(parsed)) {
        const profiles = parsed.filter(isProviderProfile).map((profile) => ({
          id: profile.id.trim(),
          name: profile.name.trim(),
          endpoint: profile.endpoint.trim(),
          model: profile.model.trim(),
        }))
        if (profiles.length > 0) return profiles
      }
    }
  } catch {
    // Provider preferences are best-effort; built-ins remain available.
  }
  return defaultAiProviders.map((profile) => ({ ...profile }))
}

export function saveAiProviders(providers: AiProviderProfile[]): void {
  try {
    localStorage.setItem(AI_PROVIDERS_KEY, JSON.stringify(providers.map((provider) => ({
      id: provider.id,
      name: provider.name,
      endpoint: provider.endpoint,
      model: provider.model,
    }))))
  } catch {
    // Keep the active provider list in memory when persistence is unavailable.
  }
}
