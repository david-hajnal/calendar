# Architecture: dark/light theme

## Fit
- `frontend/src/styles.css` — add `[data-theme="dark"]` variable overrides
- `frontend/src/App.tsx` — add theme toggle button to `app-header__actions`
- New file: `frontend/src/theme/` — theme context provider + toggle hook
- No backend changes, no API endpoints, no database changes

## Endpoints
none

## Data
none

## Flow
1. `main.tsx` renders `<App />`
2. `App` renders `<AuthProvider>` (existing)
3. New `<ThemeProvider>` wraps children in `<App>` (outside AuthProvider)
4. `ThemeProvider` reads `localStorage` on mount, falls back to `prefers-color-scheme`, sets `data-theme` on `<html>`
5. `AuthenticatedShell` header gets toggle button via `useTheme()` hook
6. Toggle clicks `useTheme().setTheme()` → writes `localStorage` → sets `data-theme` on `<html>` → triggers CSS variable swap

## External
- `prefers-color-scheme` media query for system preference detection
- `localStorage` key: `theme` (values: `"light"`, `"dark"`, `"system"`)
