import { render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";

import { NotificationSurface } from "./NotificationSurface";

it("renders authenticated in-app notifications", async () => {
  const api = { request: vi.fn().mockResolvedValue(new Response(JSON.stringify([{ id: 1, event_id: 4, event_title: "Planning", created_at: 1, read_at: null }]), { status: 200 })) } as never;
  render(<NotificationSurface api={api} />);
  expect(await screen.findByRole("status", { name: "Notifications" })).toHaveTextContent("Planning");
});
