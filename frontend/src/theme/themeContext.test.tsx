import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ThemeProvider, useTheme } from "./themeContext";

const THEME_KEY = "theme";

function ThemeDisplay() {
  const { mode, resolvedTheme } = useTheme();
  return (
    <output data-testid="theme">
      {mode}:{resolvedTheme}
    </output>
  );
}

function ToggleButton() {
  const { toggle, resolvedTheme } = useTheme();
  return <button onClick={toggle}>Toggle ({resolvedTheme})</button>;
}

describe("ThemeProvider", () => {
  beforeEach(() => {
    vi.stubGlobal("matchMedia", vi.fn().mockImplementation(() => ({
      matches: false,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })));
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    vi.clearAllMocks();
    vi.restoreAllMocks();
  });

  it("renders with light theme when no storage and system prefers light", () => {
    render(<ThemeProvider><ThemeDisplay /></ThemeProvider>);
    expect(screen.getByTestId("theme")).toHaveTextContent("system:light");
  });

  it("renders with dark theme when system prefers dark", () => {
    vi.mocked(window.matchMedia).mockImplementation(() => ({
      matches: true,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }));
    render(<ThemeProvider><ThemeDisplay /></ThemeProvider>);
    expect(screen.getByTestId("theme")).toHaveTextContent("system:dark");
  });

  it("reads existing localStorage value on mount", () => {
    const original = window.localStorage;
    Object.defineProperty(window, "localStorage", {
      value: { getItem: vi.fn(() => "dark"), setItem: vi.fn(), removeItem: vi.fn() },
      writable: true,
    });
    render(<ThemeProvider><ThemeDisplay /></ThemeProvider>);
    expect(screen.getByTestId("theme")).toHaveTextContent("dark:dark");
    Object.defineProperty(window, "localStorage", { value: original });
  });

  it("resolves to dark when mode is system and system changes", async () => {
    let matches = false;
    const listeners: Array<() => void> = [];
    const mockMediaQuery = {
      get matches() { return matches; },
      addEventListener: (_: string, cb: () => void) => { listeners.push(cb); },
      removeEventListener: vi.fn(),
    };
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue(mockMediaQuery));
    const original = window.localStorage;
    Object.defineProperty(window, "localStorage", {
      value: { getItem: vi.fn(() => "system"), setItem: vi.fn(), removeItem: vi.fn() },
      writable: true,
    });
    render(<ThemeProvider><ThemeDisplay /></ThemeProvider>);
    expect(screen.getByTestId("theme")).toHaveTextContent("system:light");

    matches = true;
    listeners[0]();
    await waitFor(() => expect(screen.getByTestId("theme")).toHaveTextContent("system:dark"));
    Object.defineProperty(window, "localStorage", { value: original });
  });

  it("toggle switches light to dark and persists to localStorage", () => {
    const setItem = vi.fn();
    const original = window.localStorage;
    Object.defineProperty(window, "localStorage", {
      value: { getItem: vi.fn(() => null), setItem, removeItem: vi.fn() },
      writable: true,
    });
    render(<ThemeProvider><ToggleButton /></ThemeProvider>);
    const button = screen.getByRole("button");
    expect(button).toHaveTextContent("Toggle (light)");
    button.click();
    expect(setItem).toHaveBeenCalledWith(THEME_KEY, "dark");
    Object.defineProperty(window, "localStorage", { value: original });
  });

  it("toggle switches dark to light and persists to localStorage", () => {
    const setItem = vi.fn();
    const original = window.localStorage;
    Object.defineProperty(window, "localStorage", {
      value: { getItem: vi.fn(() => "dark"), setItem, removeItem: vi.fn() },
      writable: true,
    });
    render(<ThemeProvider><ToggleButton /></ThemeProvider>);
    const button = screen.getByRole("button");
    expect(button).toHaveTextContent("Toggle (dark)");
    button.click();
    expect(setItem).toHaveBeenCalledWith(THEME_KEY, "light");
    Object.defineProperty(window, "localStorage", { value: original });
  });
});
