// Apply the saved theme before first paint, to avoid a flash. This is a same-origin file,
// because the production Content-Security-Policy allows scripts from 'self' only.
try {
  var stored = localStorage.getItem('ask_theme');
  var dark = stored ? stored === 'dark' : window.matchMedia('(prefers-color-scheme: dark)').matches;
  if (dark) document.documentElement.classList.add('dark');
} catch (e) {
  // Storage can be blocked. The default light theme is fine.
}
