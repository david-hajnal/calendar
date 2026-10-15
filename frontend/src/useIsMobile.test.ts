import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { MOBILE_QUERY } from "./useIsMobile";

function lengthToPx(value: string): number {
  const match = value.trim().match(/^([\d.]+)(px|rem)$/);
  if (!match) throw new Error(`Cannot parse CSS length: "${value}"`);
  const amount = Number(match[1]);
  return match[2] === "rem" ? amount * 16 : amount;
}

describe("useIsMobile breakpoint", () => {
  it("matches the CSS mobile breakpoint in CalendarEventUI.css", () => {
    const jsMatch = MOBILE_QUERY.match(/max-width:\s*([\d.]+(?:px|rem))/);
    expect(jsMatch, `Could not parse max-width from MOBILE_QUERY: ${MOBILE_QUERY}`).not.toBeNull();
    const jsMobilePx = lengthToPx(jsMatch![1]);

    const dir = path.dirname(fileURLToPath(import.meta.url));
    const cssPath = path.join(dir, "calendar", "CalendarEventUI.css");
    const css = readFileSync(cssPath, "utf8");
    const cssMatch = css.match(/\/\*\s*Mobile styles\s*\*\/\s*@media\s*\(max-width:\s*([\d.]+(?:px|rem))\)/);
    expect(cssMatch, 'Could not find the "Mobile styles" media query in CalendarEventUI.css').not.toBeNull();
    const cssMobilePx = lengthToPx(cssMatch![1]);

    // The JS hook and the CSS must agree on where "mobile" ends. If they differ,
    // the component renders the mobile layout at a viewport width where the CSS
    // still applies the tablet styles — a 1px seam at the breakpoint.
    expect(jsMobilePx).toBe(cssMobilePx);
  });
});
