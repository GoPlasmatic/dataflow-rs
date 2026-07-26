import js from '@eslint/js'
import globals from 'globals'
import tseslint from 'typescript-eslint'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'

export default tseslint.config(
  {
    ignores: ['dist', 'eslint.config.js'],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      globals: globals.browser,
    },
    plugins: {
      'react-hooks': reactHooks,
      'react-refresh': reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // `ui/` ships as a component library, not an app relying on Vite fast
      // refresh. Context modules exporting a Provider component alongside its
      // `useX` hook is the intended public shape here, so this rule reports
      // noise rather than a defect.
      'react-refresh/only-export-components': 'off',
      // The public surface intentionally re-exports types alongside
      // components; the unused-vars check is handled by tsc's
      // `noUnusedLocals`/`noUnusedParameters`, which run in `build:lib`.
      // Keep the lint rule but let a leading underscore opt out.
      '@typescript-eslint/no-unused-vars': [
        'error',
        {
          argsIgnorePattern: '^_',
          varsIgnorePattern: '^_',
          caughtErrorsIgnorePattern: '^_',
        },
      ],
    },
  },
  // Build tooling runs under Node, not the browser.
  {
    files: ['scripts/**/*.mjs'],
    languageOptions: {
      globals: globals.node,
    },
  },
)
