import { useState, useEffect } from 'react'

export interface Tweaks {
  theme: 'light' | 'dark'
  navStyle: 'sidebar' | 'rail' | 'topbar'
  flagsLayout: 'table' | 'cards' | 'grouped'
  flagDetailLayout: 'stacked' | 'side'
  expViz: 'auto' | 'frequentist' | 'bayesian'
  density: 'comfortable' | 'compact'
  accent: string
}

const DEFAULTS: Tweaks = {
  theme: 'light',
  navStyle: 'sidebar',
  flagsLayout: 'table',
  flagDetailLayout: 'stacked',
  expViz: 'auto',
  density: 'comfortable',
  accent: '#F0461F',
}

const STORAGE_KEY = 'stitchd_tweaks'

function loadTweaks(): Tweaks {
  try {
    const stored = localStorage.getItem(STORAGE_KEY)
    if (stored) return { ...DEFAULTS, ...JSON.parse(stored) }
  } catch {
    // ignore
  }
  return DEFAULTS
}

export function useTweaks() {
  const [tweaks, setTweaks] = useState<Tweaks>(loadTweaks)

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', tweaks.theme)
    document.documentElement.style.setProperty('--accent', tweaks.accent)
    document.body.dataset.density = tweaks.density
    localStorage.setItem(STORAGE_KEY, JSON.stringify(tweaks))
  }, [tweaks])

  function setTweak<K extends keyof Tweaks>(key: K, value: Tweaks[K]) {
    setTweaks((t) => ({ ...t, [key]: value }))
  }

  return { tweaks, setTweak }
}
