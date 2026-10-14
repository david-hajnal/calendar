import "@testing-library/jest-dom/vitest";

// Pin the timezone so timezone-sensitive tests (day-boundary clipping, scroll
// anchoring) are deterministic regardless of the machine or CI runner's local
// timezone. The calendar renders events in the viewer's local timezone, and the
// test fixtures are authored against Europe/Budapest (UTC+2 in summer).
process.env.TZ = "Europe/Budapest";
