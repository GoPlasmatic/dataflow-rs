import { useEffect, useCallback } from 'react';

interface KeyboardShortcutHandlers {
  onTogglePanel?: () => void;
  onFormatJson?: () => void;
}

export function useKeyboardShortcuts(handlers: KeyboardShortcutHandlers) {
  const handleKeyDown = useCallback(
    (event: KeyboardEvent) => {
      const isMac = navigator.platform.toUpperCase().indexOf('MAC') >= 0;
      const modKey = isMac ? event.metaKey : event.ctrlKey;

      // Ctrl/Cmd + B - Toggle panel
      if (modKey && event.key === 'b') {
        event.preventDefault();
        handlers.onTogglePanel?.();
      }

      // Ctrl/Cmd + Shift + F - Format JSON
      if (modKey && event.shiftKey && event.key === 'f') {
        event.preventDefault();
        handlers.onFormatJson?.();
      }
    },
    [handlers]
  );

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);
}
