import { useEffect, useCallback } from 'react';

interface KeyboardShortcutHandlers {
  onTogglePanel?: () => void;
  onFormatJson?: () => void;
}

export function useKeyboardShortcuts({ onTogglePanel, onFormatJson }: KeyboardShortcutHandlers) {
  const handleKeyDown = useCallback(
    (event: KeyboardEvent) => {
      const isMac = navigator.platform.toUpperCase().includes('MAC');
      const modKey = isMac ? event.metaKey : event.ctrlKey;
      const key = event.key.toLowerCase();

      // Ctrl/Cmd + B - Toggle panel
      if (modKey && !event.shiftKey && key === 'b') {
        event.preventDefault();
        onTogglePanel?.();
      }

      // Ctrl/Cmd + Shift + F - Format JSON
      if (modKey && event.shiftKey && key === 'f') {
        event.preventDefault();
        onFormatJson?.();
      }
    },
    [onTogglePanel, onFormatJson]
  );

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);
}
