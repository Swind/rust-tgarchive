export type Theme = 'light' | 'dark';
const KEY = 'tgarchive.theme';

export function storedTheme(): Theme | null {
  try {
    const v = localStorage.getItem(KEY);
    return v === 'light' || v === 'dark' ? v : null;
  } catch {
    return null;
  }
}

export function currentTheme(): Theme {
  return storedTheme() ?? (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
}

export function applyStoredTheme() {
  const stored = storedTheme();
  if (stored) document.documentElement.dataset.theme = stored;
}

export function setTheme(theme: Theme) {
  document.documentElement.dataset.theme = theme;
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    /* storage unavailable: theme only lasts for this session */
  }
}
