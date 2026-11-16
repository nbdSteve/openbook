import js from '@eslint/js';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  { ignores: ['node_modules/', 'cdk.out/', 'dist/', 'iam/rendered/', 'test/fixtures/'] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    rules: {
      // No `any` without a comment that says why (see AGENTS.md).
      '@typescript-eslint/no-explicit-any': 'error',
    },
  },
);
