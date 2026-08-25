import { useRef, useCallback, useEffect } from 'react';
import Editor, { OnMount, BeforeMount } from '@monaco-editor/react';
import type { editor } from 'monaco-editor';
import { findPathLineNumbers } from '../../utils';
import { SIGNAL_BOARD, bare } from './signalBoardTokens';

interface JsonEditorProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  readOnly?: boolean;
  className?: string;
  theme?: 'light' | 'dark';
  onCursorChange?: (line: number, column: number) => void;
  /** Paths to highlight in the editor (e.g., ["data.user.name", "context.metadata"]) */
  highlightedPaths?: string[];
}

/**
 * Signal Board Monaco themes.
 *
 * JSON syntax is coloured by the SIGNAL rule — the kind of value each token
 * denotes — so the editor reads in the same colour language as the workflow
 * tree and the flow diagram: keys tap into data (teal), string values are the
 * string signal (amber), numbers the number signal (blue), and the literals
 * true/false/null the boolean signal (red). The chrome — cursor, selection,
 * gutter, indent guides — sits on the board neutrals, with --accent reserved
 * for the cursor and selection as it is everywhere else.
 */
const signalBoardTheme = (
  t: typeof SIGNAL_BOARD.light | typeof SIGNAL_BOARD.dark,
  base: 'vs' | 'vs-dark'
) => ({
  base,
  inherit: true,
  rules: [
    { token: 'string.key.json', foreground: bare(t.sigData) },
    { token: 'string.value.json', foreground: bare(t.sigString) },
    { token: 'number', foreground: bare(t.sigNumber) },
    { token: 'keyword', foreground: bare(t.sigBoolFalse) },
    { token: 'delimiter', foreground: bare(t.muted) },
  ],
  colors: {
    'editor.background': t.surface,
    'editor.foreground': t.ink,
    'editor.lineHighlightBackground': t.surface2,
    'editor.selectionBackground': t.accentSoft,
    'editor.selectionHighlightBackground': t.surface2,
    'editorCursor.foreground': t.accent,
    'editorLineNumber.foreground': t.faint,
    'editorLineNumber.activeForeground': t.ink2,
    'editorIndentGuide.background': t.hairline2,
    'editorIndentGuide.activeBackground': t.hairline,
    'editorBracketMatch.background': t.accentSoft,
    'editorBracketMatch.border': t.accent,
    'editorGutter.background': t.surface,
    'editorWidget.background': t.surface,
    'editorWidget.border': t.hairline,
    'scrollbarSlider.background': `${t.faint}59`,
    'scrollbarSlider.hoverBackground': `${t.muted}99`,
    'scrollbarSlider.activeBackground': `${t.muted}cc`,
    'minimap.background': t.surface,
  },
});

const defineThemes: BeforeMount = (monaco) => {
  monaco.editor.defineTheme(
    'signal-board-dark',
    signalBoardTheme(SIGNAL_BOARD.dark, 'vs-dark')
  );
  monaco.editor.defineTheme(
    'signal-board-light',
    signalBoardTheme(SIGNAL_BOARD.light, 'vs')
  );
};

export function JsonEditor({
  value,
  onChange,
  readOnly = false,
  className = '',
  theme = 'dark',
  onCursorChange,
  highlightedPaths,
}: JsonEditorProps) {
  const editorRef = useRef<editor.IStandaloneCodeEditor | null>(null);
  const decorationsRef = useRef<string[]>([]);

  const handleEditorMount: OnMount = useCallback((editor, monaco) => {
    editorRef.current = editor;

    // Configure JSON validation
    monaco.languages.json.jsonDefaults.setDiagnosticsOptions({
      validate: true,
      schemas: [],
      allowComments: false,
      trailingCommas: 'error',
    });

    // Add cursor position listener
    if (onCursorChange) {
      editor.onDidChangeCursorPosition((e) => {
        onCursorChange(e.position.lineNumber, e.position.column);
      });
    }

    // Focus the editor only if not readOnly
    if (!readOnly) {
      editor.focus();
    }
  }, [onCursorChange, readOnly]);

  // Apply line decorations for highlighted paths
  useEffect(() => {
    if (!editorRef.current || !highlightedPaths || highlightedPaths.length === 0) {
      // Clear decorations if no paths
      if (editorRef.current && decorationsRef.current.length > 0) {
        decorationsRef.current = editorRef.current.deltaDecorations(decorationsRef.current, []);
      }
      return;
    }

    const lineNumbers = findPathLineNumbers(value, highlightedPaths);

    if (lineNumbers.length > 0) {
      const decorations: editor.IModelDeltaDecoration[] = lineNumbers.map(lineNumber => ({
        range: {
          startLineNumber: lineNumber,
          startColumn: 1,
          endLineNumber: lineNumber,
          endColumn: 1,
        },
        options: {
          isWholeLine: true,
          className: 'df-highlighted-line',
          glyphMarginClassName: 'df-highlighted-glyph',
          overviewRuler: {
            color:
              theme === 'dark'
                ? SIGNAL_BOARD.dark.sigBoolTrue
                : SIGNAL_BOARD.light.sigBoolTrue,
            position: 1, // Left
          },
        },
      }));

      decorationsRef.current = editorRef.current.deltaDecorations(
        decorationsRef.current,
        decorations
      );
    } else {
      // Clear decorations
      decorationsRef.current = editorRef.current.deltaDecorations(decorationsRef.current, []);
    }
  }, [value, highlightedPaths, theme]);

  const handleChange = useCallback((newValue: string | undefined) => {
    onChange(newValue || '');
  }, [onChange]);

  return (
    <div className={`df-monaco-editor-wrapper ${className}`}>
      <Editor
        height="100%"
        defaultLanguage="json"
        value={value}
        onChange={handleChange}
        onMount={handleEditorMount}
        beforeMount={defineThemes}
        theme={theme === 'dark' ? 'signal-board-dark' : 'signal-board-light'}
        options={{
          readOnly,
          minimap: { enabled: false },
          fontSize: 13,
          // Mirrors --font-mono. Monaco needs a literal stack, not a var().
          fontFamily:
            "'JetBrains Mono', ui-monospace, 'SF Mono', 'Cascadia Code', 'Consolas', monospace",
          lineHeight: 20,
          tabSize: 2,
          insertSpaces: true,
          automaticLayout: true,
          scrollBeyondLastLine: false,
          wordWrap: 'on',
          wrappingIndent: 'indent',
          folding: true,
          foldingStrategy: 'indentation',
          showFoldingControls: 'mouseover',
          bracketPairColorization: { enabled: true },
          guides: {
            bracketPairs: false,
            indentation: false,
            highlightActiveBracketPair: true,
            highlightActiveIndentation: false,
          },
          renderLineHighlight: readOnly ? 'none' : 'line',
          selectOnLineNumbers: true,
          roundedSelection: true,
          cursorBlinking: 'smooth',
          cursorSmoothCaretAnimation: 'on',
          smoothScrolling: true,
          padding: { top: 8, bottom: 8 },
          scrollbar: {
            vertical: 'auto',
            horizontal: 'auto',
            verticalScrollbarSize: 10,
            horizontalScrollbarSize: 10,
          },
          overviewRulerBorder: false,
          hideCursorInOverviewRuler: true,
          contextmenu: true,
          quickSuggestions: false,
          suggestOnTriggerCharacters: false,
          acceptSuggestionOnEnter: 'off',
          formatOnPaste: true,
          formatOnType: false,
          glyphMargin: false,
        }}
      />
    </div>
  );
}
