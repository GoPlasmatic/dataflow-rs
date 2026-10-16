// Initializes @goplasmatic/dataflow-wasm before any test runs. The package is
// built for the web, where `init()` fetches the binary by URL; under node the
// bytes are read from the installed package instead. Every test file shares
// one instance, which is what a page has.
//
// The path is taken from the project root, not `import.meta.url`: under the
// jsdom environment that URL is the page's `http://localhost/`, not a file.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import init from '@goplasmatic/dataflow-wasm';

const wasm = resolve(process.cwd(), 'node_modules/@goplasmatic/dataflow-wasm/dataflow_wasm_bg.wasm');
await init({ module_or_path: readFileSync(wasm) });

// jsdom has no `matchMedia`; the theme provider reads it for the system
// theme. A stub that reports no match keeps the components on the light theme.
if (typeof window !== 'undefined' && typeof window.matchMedia !== 'function') {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: (query: string): MediaQueryList =>
      ({
        matches: false,
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
      }) as MediaQueryList,
  });
}

// jsdom has no `ResizeObserver` either; the panels that size themselves to
// their container observe it. A stub that never fires leaves them at their
// initial size, which is all a test reads.
if (typeof globalThis.ResizeObserver === 'undefined') {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  Object.defineProperty(globalThis, 'ResizeObserver', { writable: true, value: ResizeObserverStub });
}

// Nor `scrollIntoView`; the tree scrolls the current step into view.
if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {};
}
