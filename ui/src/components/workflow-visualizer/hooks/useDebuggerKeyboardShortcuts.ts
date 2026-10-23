import { useEffect, useCallback } from 'react';
import type { PlaybackState } from '../../../types';

interface DebuggerKeyboardShortcutsOptions {
  playbackState: PlaybackState;
  hasTrace: boolean;
  isAtStart: boolean;
  isAtEnd: boolean;
  play: () => void;
  pause: () => void;
  stepForward: () => void;
  stepBackward: () => void;
  onStop?: () => void;
  onFirst?: () => void;
  onLast?: () => void;
}

/**
 * Shared keyboard shortcuts for debugger playback controls.
 * Ignores key events inside interactive elements, text inputs, and code/logic editors.
 */
export function useDebuggerKeyboardShortcuts({
  playbackState,
  hasTrace,
  isAtStart,
  isAtEnd,
  play,
  pause,
  stepForward,
  stepBackward,
  onStop,
  onFirst,
  onLast,
}: DebuggerKeyboardShortcutsOptions) {
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      // Ignore keyboard shortcuts inside text inputs, interactive controls, or the embedded DataLogicEditor / Monaco editor
      if (e.target instanceof HTMLElement) {
        if (
          e.target instanceof HTMLInputElement ||
          e.target instanceof HTMLTextAreaElement ||
          e.target instanceof HTMLSelectElement ||
          e.target instanceof HTMLButtonElement ||
          e.target.isContentEditable ||
          e.target.closest('.logic-editor, .monaco-editor, button, a[href], [role="textbox"], [role="treeitem"]')
        ) {
          return;
        }
      }

      switch (e.key) {
        case ' ':
          e.preventDefault();
          if (playbackState === 'playing') {
            pause();
          } else if (hasTrace && !isAtEnd) {
            play();
          }
          break;
        case 'ArrowRight':
          e.preventDefault();
          if (hasTrace && !isAtEnd) {
            stepForward();
          }
          break;
        case 'ArrowLeft':
          e.preventDefault();
          if (hasTrace && !isAtStart) {
            stepBackward();
          }
          break;
        case 'Escape':
          if (onStop) {
            e.preventDefault();
            onStop();
          }
          break;
        case 'Home':
          if (onFirst) {
            e.preventDefault();
            if (hasTrace) {
              onFirst();
            }
          }
          break;
        case 'End':
          if (onLast) {
            e.preventDefault();
            if (hasTrace) {
              onLast();
            }
          }
          break;
      }
    },
    [
      playbackState,
      hasTrace,
      isAtEnd,
      isAtStart,
      play,
      pause,
      stepForward,
      stepBackward,
      onStop,
      onFirst,
      onLast,
    ]
  );

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);
}
