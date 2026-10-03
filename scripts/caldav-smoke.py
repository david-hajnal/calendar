#!/usr/bin/env python3
"""Read-only CalDAV semantic smoke check. Credentials come only from environment."""
import base64
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET

DAV = "DAV:"
CAL = "urn:ietf:params:xml:ns:caldav"
APPLE = "http://apple.com/ns/ical/"
MAX_BYTES = 16 * 1024 * 1024


def tag(namespace, name):
    return f"{{{namespace}}}{name}"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def main():
    origin = os.environ["CALDAV_ORIGIN"].rstrip("/")
    parsed = urllib.parse.urlsplit(origin)
    if parsed.scheme not in ("https", "http") or parsed.username or parsed.password or parsed.path:
        raise ValueError("CALDAV_ORIGIN must contain only scheme and host")
    if parsed.scheme != "https" and parsed.hostname not in ("localhost", "127.0.0.1", "::1"):
        raise ValueError("remote smoke checks require HTTPS")
    authorization = "Basic " + base64.b64encode(
        (os.environ["CALDAV_USERNAME"] + ":" + os.environ["CALDAV_PASSWORD"]).encode()
    ).decode()
    opener = urllib.request.build_opener(NoRedirect)

    def url(href):
        result = urllib.parse.urljoin(origin + "/", href)
        target = urllib.parse.urlsplit(result)
        if (target.scheme, target.netloc) != (parsed.scheme, parsed.netloc) or target.username or target.password:
            raise ValueError("advertised URL leaves configured origin")
        return result

    def request(href, method, body=None, depth=None):
        headers = {"Authorization": authorization, "Content-Type": "application/xml; charset=utf-8",
                   "User-Agent": "Happening-CalDAV-Smoke/1.0"}
        if depth is not None:
            headers["Depth"] = str(depth)
        req = urllib.request.Request(url(href), data=body.encode() if body is not None else None,
                                     headers=headers, method=method)
        try:
            response = opener.open(req, timeout=30)
        except urllib.error.HTTPError as error:
            response = error
        payload = response.read(MAX_BYTES + 1)
        if len(payload) > MAX_BYTES:
            raise ValueError("response exceeds smoke-check bound")
        return response.status, response.headers, payload

    def xml_response(href, method, body, depth):
        status, _, payload = request(href, method, body, depth)
        if status != 207:
            raise ValueError(f"{method} failed with status {status}")
        # ElementTree refuses undefined entities; prohibit DTDs explicitly.
        if b"<!DOCTYPE" in payload.upper():
            raise ValueError("DAV response contains a DTD")
        root = ET.fromstring(payload)
        if root.tag != tag(DAV, "multistatus"):
            raise ValueError("DAV response root is not multistatus")
        for item in root.findall(tag(DAV, "response")):
            hrefs = item.findall(tag(DAV, "href"))
            if len(hrefs) != 1 or not hrefs[0].text:
                raise ValueError("response has no unique href")
            if not item.findall(tag(DAV, "propstat")) and item.find(tag(DAV, "status")) is None:
                raise ValueError("response has no status")
            for group in item.findall(tag(DAV, "propstat")):
                prop = group.find(tag(DAV, "prop"))
                code = group.findtext(tag(DAV, "status"), "")
                if prop is None or not len(prop) or (prop.text or "").strip():
                    raise ValueError("empty propstat or bare property-name text")
                if code not in ("HTTP/1.1 200 OK", "HTTP/1.1 404 Not Found"):
                    raise ValueError("unexpected property status")
                if "404" in code and any(len(child) or child.text for child in prop):
                    raise ValueError("missing property contains fabricated values")
        return root

    def propfind(href, names, depth=0):
        prop = ET.Element(tag(DAV, "propfind"))
        selection = ET.SubElement(prop, tag(DAV, "prop"))
        for namespace, name in names:
            ET.SubElement(selection, tag(namespace, name))
        return xml_response(href, "PROPFIND", ET.tostring(prop, encoding="unicode"), depth)

    def properties(item):
        values = {}
        for group in item.findall(tag(DAV, "propstat")):
            status = group.findtext(tag(DAV, "status"))
            for value in group.find(tag(DAV, "prop")):
                if value.tag in values:
                    raise ValueError("duplicate returned property")
                values[value.tag] = (status, value)
        return values

    def required(item, namespace, name):
        status, value = properties(item).get(tag(namespace, name), (None, None))
        if status != "HTTP/1.1 200 OK":
            raise ValueError(f"required property {name} did not succeed")
        return value

    print("Checking GET and PROPFIND well-known discovery...", flush=True)
    discovery_body = '<D:propfind xmlns:D="DAV:"><D:prop><D:current-user-principal/><D:resourcetype/></D:prop></D:propfind>'
    targets = []
    for method in ("GET", "PROPFIND"):
        status, headers, _ = request("/.well-known/caldav", method,
                                     discovery_body if method == "PROPFIND" else None,
                                     0 if method == "PROPFIND" else None)
        allowed = (307, 308) if method == "PROPFIND" else (301, 302, 307, 308)
        if status not in allowed or not headers.get("Location"):
            challenge = headers.get("cf-mitigated", "").lower() == "challenge"
            raise ValueError(f"{method} discovery failed (HTTP {status}; Location present: {bool(headers.get('Location'))}; Cloudflare challenge: {challenge})")
        target = url(headers["Location"])
        if target != origin + "/dav/":
            raise ValueError("discovery target is not canonical /dav/")
        targets.append(target)
    if targets[0] != targets[1]:
        raise ValueError("GET and PROPFIND discovery targets differ")
    print("Checking DAV root after method/body-preserving discovery...", flush=True)
    root = xml_response(targets[1], "PROPFIND", discovery_body, 0)
    item = root.find(tag(DAV, "response"))
    principal = required(item, DAV, "current-user-principal").findtext(tag(DAV, "href"))
    print("Checking principal properties...", flush=True)
    principal_result = propfind(principal, [(CAL, "calendar-home-set"), (DAV, "resourcetype"),
                                          (DAV, "current-user-principal"), ("urn:happening:smoke&extension", "unsupported")])
    item = principal_result.find(tag(DAV, "response"))
    missing = properties(item).get(tag("urn:happening:smoke&extension", "unsupported"))
    if missing is None or missing[0] != "HTTP/1.1 404 Not Found":
        raise ValueError("extension property did not preserve its namespace/status")
    if required(item, DAV, "resourcetype").find(tag(DAV, "principal")) is None:
        raise ValueError("principal resource type missing")
    home = required(item, CAL, "calendar-home-set").findtext(tag(DAV, "href"))
    print("Checking calendar-home and collection properties...", flush=True)
    home_result = propfind(home, [(DAV, "resourcetype"), (DAV, "current-user-principal"),
                                (DAV, "supported-report-set"), (CAL, "supported-calendar-component-set"),
                                (CAL, "supported-calendar-data"), (DAV, "sync-token"), (APPLE, "calendar-color")], 1)
    calendars = 0
    for item in home_result.findall(tag(DAV, "response")):
        resource_type = required(item, DAV, "resourcetype")
        if resource_type.find(tag(CAL, "calendar")) is None:
            continue
        calendars += 1
        if resource_type.find(tag(DAV, "collection")) is None:
            raise ValueError("calendar is not a DAV collection")
        required(item, DAV, "current-user-principal")
        reports = required(item, DAV, "supported-report-set")
        expected = {tag(CAL, "calendar-query"), tag(CAL, "calendar-multiget"), tag(DAV, "sync-collection")}
        actual = {child.tag for wrapper in reports.findall(tag(DAV, "supported-report"))
                  for report in wrapper.findall(tag(DAV, "report")) for child in report}
        if not expected <= actual:
            raise ValueError("supported-report-set has incorrect hierarchy/namespaces")
        href = item.findtext(tag(DAV, "href"))
        required(item, CAL, "supported-calendar-component-set")
        required(item, CAL, "supported-calendar-data")
        token = required(item, DAV, "sync-token").text
        sync = ET.Element(tag(DAV, "sync-collection"))
        # Empty first snapshot is paged. Follow continuations until complete.
        ET.SubElement(sync, tag(DAV, "sync-token"))
        ET.SubElement(sync, tag(DAV, "sync-level")).text = "1"
        selection = ET.SubElement(sync, tag(DAV, "prop"))
        ET.SubElement(selection, tag(DAV, "getetag"))
        print(f"Checking initial sync for calendar {calendars}...", flush=True)
        for page in range(1000):
            result = xml_response(href, "REPORT", ET.tostring(sync, encoding="unicode"), 0)
            tokens = result.findall(tag(DAV, "sync-token"))
            if len(tokens) != 1 or not tokens[0].text or not urllib.parse.urlsplit(tokens[0].text).scheme:
                raise ValueError("sync-token must be a direct, URI-valued child")
            continued = any(item.findtext(tag(DAV, "status")) == "HTTP/1.1 507 Insufficient Storage"
                            for item in result.findall(tag(DAV, "response")))
            if not continued:
                break
            if sync.find(tag(DAV, "sync-token")).text == tokens[0].text:
                raise ValueError("sync continuation token did not advance")
            sync.find(tag(DAV, "sync-token")).text = tokens[0].text
        else:
            raise ValueError("sync continuation exceeded smoke-check bound")
    print(f"CalDAV XML discovery and initial sync passed ({calendars} calendars). macOS setup remains a separate check.")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, ValueError, ET.ParseError, urllib.error.URLError, TimeoutError) as error:
        # Never dump request objects, headers, URLs with credentials or passwords.
        # ValueError messages here are controlled validation descriptions.
        # Network exceptions can embed URLs; keep those and missing env keys generic.
        detail = f": {error}" if type(error) is ValueError else ""
        print(f"CalDAV smoke failed: {type(error).__name__}{detail}", file=sys.stderr)
        sys.exit(1)
