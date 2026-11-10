import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";

export default tseslint.config(
  { ignores: ["dist", "node_modules", "src-tauri"] },
  js.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  ...tseslint.configs.stylisticTypeChecked,
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname },
    },
    plugins: { "react-hooks": reactHooks },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // Tauri's `invoke` returns `Promise<unknown>`-ish generics; explicit casts at the boundary are
      // wrapped in `src/api.ts`, so the rest of the app can stay strict.
      "@typescript-eslint/restrict-template-expressions": ["error", { allowNumber: true }],
      // `onClick={() => setX(v)}` is idiomatic React; the void return is never consumed.
      "@typescript-eslint/no-confusing-void-expression": ["error", { ignoreArrowShorthand: true }],
    },
  },
);
