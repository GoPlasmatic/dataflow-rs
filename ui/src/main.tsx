import React from 'react';
import ReactDOM from 'react-dom/client';
// Signal Board typography — self-hosted so the demo renders in Space Grotesk
// (UI) + JetBrains Mono (code) with no external CDN / CSP dependency.
// These live here, not in the library stylesheet: a consumer of
// @goplasmatic/dataflow-ui should not be forced into a font download, and
// --font-ui / --font-mono fall back cleanly when the faces are absent.
import '@fontsource/space-grotesk/400.css';
import '@fontsource/space-grotesk/500.css';
import '@fontsource/space-grotesk/600.css';
import '@fontsource/space-grotesk/700.css';
import '@fontsource/jetbrains-mono/400.css';
import '@fontsource/jetbrains-mono/500.css';
import '@fontsource/jetbrains-mono/600.css';
import '@fontsource/jetbrains-mono/700.css';
import App from './App';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
