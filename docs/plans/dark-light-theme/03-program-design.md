# Program Design: dark/light theme

## Files

### `frontend/src/theme/themeContext.tsx` (new)
Theme context provider and hook. Manages `data-theme` attribute on `<html>`, localStorage persistence, and system preference listening.

### `frontend/src/styles.css` (changed)
Add `[data-theme="dark"]` block with dark-mode overrides for all color tokens.

### `frontend/src/App.tsx` (changed)
Wrap `<AuthProvider>` with `<ThemeProvider>`. Add toggle button to `app-header__actions` in `AuthenticatedShell`.

## Types & signatures

```typescript
type ThemeMode = "light" | "dark" | "system";
type ResolvedTheme = "light" | "dark";

interface ThemeContextValue {
  mode: ThemeMode;          // user's stored preference
  resolvedTheme: ResolvedTheme; // actual computed theme
  setMode: (mode: ThemeMode) => void;
  toggle: () => void;       // cycle: light → dark → light
}
```

### `themeContext.tsx` exports

```typescript
export function ThemeProvider({ children }: { children: ReactNode }): JSX.Element;
export function useTheme(): ThemeContextValue;
```

### `ThemeProvider` init logic

1. Read `localStorage.getItem("theme")` → `mode`
2. If null → check `window.matchMedia("(prefers-color-scheme: dark)").matches` → default `"system"`
3. Resolve initial theme: `"system"` → media query result, else `mode`
4. Set `document.documentElement.setAttribute("data-theme", resolvedTheme)`
5. Subscribe to `prefers-color-scheme` change → update `data-theme` reactively

### `useTheme().toggle()` logic

- If `mode === "light"` → `setMode("dark")`
- If `mode === "dark"` → `setMode("light")`

(No "system" cycling — toggle is explicit light/dark switch.)

## Call stack

### Render path
1. `main.tsx` → `<App />`
2. `App.tsx` → `<ThemeProvider><AuthProvider><AuthRoutes /></AuthProvider></ThemeProvider>`
3. `ThemeProvider` sets `data-theme` on `<html>` on mount
4. `AuthenticatedShell` renders → `useTheme()` → toggle button

### Toggle path
1. User clicks toggle button → `onClick={() => useTheme().toggle()}`
2. `toggle()` calls `setMode(newMode)` → `localStorage.setItem("theme", newMode)`
3. `setMode` updates state → re-renders → `useEffect` syncs `data-theme` → CSS variables swap

## Test plan

### `themeContext.test.tsx`
- `renders with default light theme when no storage and light system preference`
- `renders with dark theme when system prefers dark`
- `reads existing localStorage value on mount`
- `resolves to dark when mode is system and system changes`
- `toggle switches light to dark and persists to localStorage`
- `toggle switches dark to light and persists to localStorage`

## Least confident decisions

1. **Toggle vs dropdown**: Toggle button (light→dark→light) vs 3-option selector (light/dark/system). Decided on toggle for simplicity — users who want system preference can set it once and forget it.
2. **localStorage vs cookie**: localStorage chosen — no server-side rendering, no SSR concerns, no cookie size/privacy overhead.
