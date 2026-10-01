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
    //           <D:current-user-principal>
    //             <D:href>${baseURL}/dav/principals/&lt;principal_id&gt;/</D:href>
    //           </D:current-user-principal>
    //         </D:prop>
    //         <D:status>HTTP/1.1 200 OK</D:status>
    //       </D:propstat>
    //     </D:response>
    //   </D:multistatus>

    const basic = Buffer.from(`${issued.username}:${issued.clear_password}`).toString("base64");
    const davHeaders = { authorization: `Basic ${basic}` };

    // Discovery step 0: well-known redirect to the DAV root.
    const wellKnown = await request.fetch(`${baseURL}/.well-known/caldav`, {
      maxRedirects: 0,
    });
    expect(wellKnown.status()).toBe(301);
    expect(wellKnown.headers()["location"]).toBe(`${baseURL}/dav/`);

    // Discovery step 1: OPTIONS advertises the implemented DAV capabilities.
    const options = await request.fetch(`${baseURL}/dav/`, { method: "OPTIONS" });
    expect(options.status()).toBe(200);
    expect(options.headers()["dav"]).toContain("calendar-access");
    expect(options.headers()["allow"]).toContain("PROPFIND");
    expect(options.headers()["allow"]).toContain("OPTIONS");

    // Discovery step 2: PROPFIND /dav/ returns the current user principal.
    const response = await request.fetch(`${baseURL}/dav/`, {
      method: "PROPFIND",
      headers: { ...davHeaders, "content-type": "application/xml; charset=utf-8" },
      data: `<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:current-user-principal/>
  </D:prop>
</D:propfind>`,
    });
    expect(response.status()).toBe(207);
    const body = await response.text();
    expect(body).toContain("<D:current-user-principal>");
    expect(body).toContain(`/dav/principals/`);
    expect(body).toContain("HTTP/1.1 200 OK");

    const principalHref = /<D:current-user-principal>\s*<D:href>([^<]+)<\/D:href>/.exec(body)?.[1];
    expect(principalHref, "current-user-principal href should be present").toBeTruthy();

    // Discovery step 3: PROPFIND the principal to find the calendar home.
    const principal = await request.fetch(principalHref as string, {
      method: "PROPFIND",
      headers: davHeaders,
    });
    expect(principal.status()).toBe(207);
    const principalBody = await principal.text();
    expect(principalBody).toContain("<D:principal-URL>");
    expect(principalBody).toContain("<D:calendar-home-set>");
    expect(principalBody).toContain("/dav/calendars/");
    expect(principalBody).toContain("calendar-query");
    expect(principalBody).toContain("calendar-multiget");
    expect(principalBody).toContain("sync-collection");

    const unauthenticated = await request.fetch(`${baseURL}/dav/`, { method: "PROPFIND" });
    expect(unauthenticated.status()).toBe(401);
    expect(unauthenticated.headers()["www-authenticate"]).toContain("Basic");
  });
});
