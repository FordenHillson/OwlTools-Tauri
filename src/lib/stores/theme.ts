import { writable } from 'svelte/store';

type Theme = 'light' | 'dark';

function getInitial(): Theme {
  try {
    const saved = localStorage.getItem('theme') as Theme | null;
    if (saved === 'light' || saved === 'dark') return saved;
  } catch {}
  if (typeof window !== 'undefined' && window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches) {
    return 'dark';
  }
  return 'light';
}

export const theme = writable<Theme>(getInitial());

let current: Theme;

theme.subscribe((t) => {
  current = t;
  try {
    localStorage.setItem('theme', t);
  } catch {}
  if (typeof document !== 'undefined') {
    document.body.classList.remove('theme-light', 'theme-dark');
    document.body.classList.add(t === 'dark' ? 'theme-dark' : 'theme-light');
  }
});

export function toggleTheme() {
  theme.update((t) => (t === 'dark' ? 'light' : 'dark'));
}
