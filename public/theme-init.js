// Run before the UI bundle so reopening a dark window does not flash white.
(() => {
  const dark = new Set(['dark', 'ink', 'midnight', 'plum']);
  const accents = ['blue', 'indigo', 'purple', 'pink', 'red', 'orange', 'green', 'teal', 'graphite'];
  let choice = 'light';
  let accent = 'blue';
  try {
    const saved = localStorage.getItem('kedu.theme');
    if (saved === 'system') choice = 'system';
    else if (saved) choice = dark.has(saved) ? 'dark' : 'light';
    const savedAccent = localStorage.getItem('kedu.accent');
    if (accents.includes(savedAccent)) accent = savedAccent;
  } catch { /* Storage can be unavailable in browser preview. */ }
  const mode = choice === 'system' ? (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light') : choice;
  const root = document.documentElement;
  root.dataset.theme = mode;
  root.dataset.colorMode = mode;
  root.dataset.accent = accent;
  root.style.colorScheme = mode;
  root.style.backgroundColor = mode === 'dark' ? '#161618' : '#f5f5f7';
})();
