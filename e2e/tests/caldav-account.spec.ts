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
  test("issues a connection password and authenticates a PROPFIND /dav/ returning the principal link", async ({ request, page }) => {
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
      headers: { ...davHeaders, depth: "0", "content-type": "application/xml; charset=utf-8" },
      data: `<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:current-user-principal/>
  </D:prop>
</D:propfind>`,
    });
    expect(response.status()).toBe(207);
    const body = await response.text();
    const principalHref = await page.evaluate((xml) => {
      const document = new DOMParser().parseFromString(xml, "application/xml");
      if (document.getElementsByTagName("parsererror").length) throw new Error("Malformed DAV XML");
      const groups = [...document.getElementsByTagNameNS("DAV:", "propstat")];
      if (groups.length !== 1 || !groups[0].getElementsByTagNameNS("DAV:", "status")[0]?.textContent?.includes("200")) throw new Error("Discovery property must succeed without empty 404 group");
      return document.getElementsByTagNameNS("DAV:", "current-user-principal")[0]?.getElementsByTagNameNS("DAV:", "href")[0]?.textContent;
    }, body);
    expect(principalHref, "current-user-principal href should be present").toBeTruthy();

    // Discovery step 3: PROPFIND the principal to find the calendar home.
    const principal = await request.fetch(principalHref as string, {
      method: "PROPFIND",
      headers: { ...davHeaders, depth: "0" },
    });
    expect(principal.status()).toBe(207);
    const principalBody = await principal.text();
    const homeHref = await page.evaluate((xml) => {
      const document = new DOMParser().parseFromString(xml, "application/xml");
      if (document.getElementsByTagName("parsererror").length) throw new Error("Malformed principal XML");
      if (!document.getElementsByTagNameNS("DAV:", "resourcetype")[0]?.getElementsByTagNameNS("DAV:", "principal").length) throw new Error("Missing principal resource type");
      return document.getElementsByTagNameNS("urn:ietf:params:xml:ns:caldav", "calendar-home-set")[0]?.getElementsByTagNameNS("DAV:", "href")[0]?.textContent;
    }, principalBody);
    expect(homeHref).toBeTruthy();
    const home = await request.fetch(homeHref as string, { method: "PROPFIND", headers: { ...davHeaders, depth: "1" } });
    expect(home.status()).toBe(207);
    const calendarHrefs = await page.evaluate((xml) => {
      const document = new DOMParser().parseFromString(xml, "application/xml");
      if (document.getElementsByTagName("parsererror").length) throw new Error("Malformed calendar home XML");
      return [...document.getElementsByTagNameNS("DAV:", "response")].filter(response => response.getElementsByTagNameNS("urn:ietf:params:xml:ns:caldav", "calendar").length > 0).map(response => response.getElementsByTagNameNS("DAV:", "href")[0].textContent!);
    }, await home.text());
    for (const href of calendarHrefs) {
      const sync = await request.fetch(href, { method: "REPORT", headers: { ...davHeaders, depth: "0" }, data: '<D:sync-collection xmlns:D="DAV:"><D:sync-token/><D:sync-level>1</D:sync-level><D:prop><D:getetag/></D:prop></D:sync-collection>' });
      expect(sync.status()).toBe(207);
      const valid = await page.evaluate((xml) => {
        const document = new DOMParser().parseFromString(xml, "application/xml");
        if (document.getElementsByTagName("parsererror").length) return false;
        const tokens = [...document.documentElement.children].filter(element => element.namespaceURI === "DAV:" && element.localName === "sync-token");
        return tokens.length === 1 && !!tokens[0].textContent;
      }, await sync.text());
      expect(valid, "sync token must be direct multistatus child").toBe(true);
    }

    const unauthenticated = await request.fetch(`${baseURL}/dav/`, { method: "PROPFIND" });
    expect(unauthenticated.status()).toBe(401);
    expect(unauthenticated.headers()["www-authenticate"]).toContain("Basic");
  });
});
