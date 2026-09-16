import { expect, test, type APIRequestContext } from "@playwright/test";

const baseURL = process.env.E2E_BASE_URL ?? "http://127.0.0.1:3100";

type Auth = { csrf: string; request: APIRequestContext };

const adminEmail = "admin@localhost";
const adminPassword = "admin-default-password-2026";

async function login(request: APIRequestContext): Promise<Auth> {
  const response = await request.post("/api/v1/auth/password-login", {
    data: { email: adminEmail, password: adminPassword },
  });
  expect(response.status(), "password login should succeed").toBe(200);
  const { csrf_token } = (await response.json()) as { csrf_token: string };
  return { csrf: csrf_token, request };
}

async function post<T>(auth: Auth, path: string, data?: unknown): Promise<T> {
  const response = await auth.request.post(path, {
    headers: {
      "x-csrf-token": auth.csrf,
      origin: baseURL,
      "sec-fetch-site": "same-origin",
    },
    data,
  });
  expect(response.status(), `POST ${path} should succeed`).toBeLessThan(300);
  return response.json() as Promise<T>;
}

interface IssuedPassword {
  server_url: string;
  username: string;
  clear_password: string;
  password: { id: number; label: string; created_at: number; last_used_at: number | null };
}

test.describe("caldav account tracer", () => {
  test("issues a connection password and authenticates a PROPFIND /dav/ returning the principal link", async ({ request }) => {
    const auth = await login(request);

    const issued = await post<IssuedPassword>(auth, "/api/v1/calendar-connections/apple/passwords", { label: "E2E DAV tracer" });
    expect(issued.clear_password).toBeTruthy();
    expect(issued.username).toBe(adminEmail);
    expect(issued.server_url).toBe(`${baseURL}/dav/`);

    // curl transcript (proof of the browser-to-API-to-DAV path):
    //
    //   $ curl -i -X PROPFIND "${baseURL}/dav/" \
    //       -H "Authorization: Basic $(printf '%s' '${issued.username}:${issued.clear_password}' | base64)"
    //
    //   HTTP/1.1 207 Multi-Status
    //   Content-Type: application/xml; charset=utf-8
    //
    //   <?xml version="1.0" encoding="utf-8" ?>
    //   <D:multistatus xmlns:D="DAV:">
    //     <D:response>
    //       <D:href>/dav/</D:href>
    //       <D:propstat>
    //         <D:prop>
    //           <D:current-principal>
    //             <D:href>${baseURL}/dav/principals/&lt;principal_id&gt;/</D:href>
    //           </D:current-principal>
    //         </D:prop>
    //         <D:status>HTTP/1.1 200 OK</D:status>
    //       </D:propstat>
    //     </D:response>
    //   </D:multistatus>

    const basic = Buffer.from(`${issued.username}:${issued.clear_password}`).toString("base64");
    const response = await request.fetch(`${baseURL}/dav/`, {
      method: "PROPFIND",
      headers: { authorization: `Basic ${basic}` },
    });
    expect(response.status()).toBe(207);
    const body = await response.text();
    expect(body).toContain("<D:current-principal>");
    expect(body).toContain(`/dav/principals/`);
    expect(body).toContain("HTTP/1.1 200 OK");

    const unauthenticated = await request.fetch(`${baseURL}/dav/`, { method: "PROPFIND" });
    expect(unauthenticated.status()).toBe(401);
    expect(unauthenticated.headers()["www-authenticate"]).toContain("Basic");
  });
});
