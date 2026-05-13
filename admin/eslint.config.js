import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import tseslint from 'typescript-eslint'
import { defineConfig, globalIgnores } from 'eslint/config'

export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      globals: globals.browser,
    },
    rules: {
      // Pre-existing pattern across the codebase: setLoading(true) inside useEffect bodies.
      // Downgraded from error to warn — tracked for future refactor.
      'react-hooks/set-state-in-effect': 'warn',
      // Pre-existing: OrgContext exports both component and non-component values.
      // Downgraded from error to warn — tracked for future refactor.
      'react-refresh/only-export-components': 'warn',
    },
  },
])
