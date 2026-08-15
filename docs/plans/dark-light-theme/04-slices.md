# Slices: dark/light theme

## Slices
- [ ] Slice 1 — tracer: `themeContext.tsx` provider + hook with light default, `ThemeProvider` wrapper in `App.tsx`, `data-theme="light"` on `<html>`, no toggle UI yet
- [ ] Slice 2 — dark CSS: `[data-theme="dark"]` block in `styles.css` with all token overrides, verified via manual inspection
- [ ] Slice 3 — toggle UI: sun/moon button in `app-header__actions`, `useTheme().toggle()` wired, localStorage persistence
- [ ] Slice 4 — system preference: initial `prefers-color-scheme` detection, `system` mode support, media query change listener
- [ ] Slice 5 — tests: `themeContext.test.tsx` with 6 test cases

## Notes
- Slice 1 proves context works end-to-end (provider renders, sets attribute)
- Slice 2 is pure CSS — no JS changes, easy to verify
- Slice 3 is the user-visible feature
- Slice 4 adds system preference (nice-to-have but important UX)
- Slice 5 adds tests after implementation is stable
