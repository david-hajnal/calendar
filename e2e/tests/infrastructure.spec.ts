import { expect, test } from "@playwright/test";

test("local application is ready before journeys run", async ({ request }) => {
  const response = await request.get("/health/ready");
  await expect(response).toBeOK();
  await expect(await response.json()).toEqual({ status: "ok" });
});
