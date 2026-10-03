#!/usr/bin/env python3
"""Local HTTP fixture regression for credential-safe semantic discovery."""
import http.server
import os
import pathlib
import subprocess
import threading
import unittest

SCRIPT = pathlib.Path(__file__).with_name('caldav-smoke.py')
BODY = b'<D:propfind xmlns:D="DAV:"><D:prop><D:current-user-principal/><D:resourcetype/></D:prop></D:propfind>'


class SmokeTest(unittest.TestCase):
    def run_fixture(self, mode):
        requests = []
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                self.handle_dav()

            def do_PROPFIND(self):
                self.handle_dav()

            def do_REPORT(self):
                self.handle_dav()

            def handle_dav(self):
                body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                requests.append((self.command, self.path, body, self.headers.get('Depth'), self.headers.get('Authorization')))
                if self.path == '/.well-known/caldav':
                    self.send_response(307)
                    self.send_header('Location', 'https://elsewhere.invalid/dav/' if mode == 'unsafe' else '/dav/')
                    self.end_headers()
                    return
                props = '<D:resourcetype><D:collection/></D:resourcetype><D:current-user-principal><D:href>/dav/principals/1/</D:href></D:current-user-principal>'
                if self.path == '/dav/principals/1/':
                    props = '<D:resourcetype><D:principal/></D:resourcetype><C:calendar-home-set><D:href>/dav/calendars/1/</D:href></C:calendar-home-set>'
                missing = '<D:propstat><D:prop><X:unsupported/></D:prop><D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>' if self.path == '/dav/principals/1/' else ''
                payload = ('<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:X="urn:happening:smoke&amp;extension"><D:response><D:href>' + self.path + '</D:href><D:propstat><D:prop>' + props + '</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>' + missing + '</D:response></D:multistatus>').encode()
                if self.path == '/dav/calendars/1/':
                    calendar_props = ('<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>'
                                      '<D:current-user-principal><D:href>/dav/principals/1/</D:href></D:current-user-principal>'
                                      '<D:supported-report-set>' + ''.join('<D:supported-report><D:report><' + report + '/></D:report></D:supported-report>' for report in ['C:calendar-query', 'C:calendar-multiget', 'D:sync-collection']) + '</D:supported-report-set>'
                                      '<C:supported-calendar-component-set><C:comp name="VEVENT"/></C:supported-calendar-component-set>'
                                      '<C:supported-calendar-data><C:calendar-data content-type="text/calendar" version="2.0"/></C:supported-calendar-data>'
                                      '<D:sync-token>urn:fixture:initial</D:sync-token>')
                    responses = ''.join('<D:response><D:href>/dav/calendars/1/' + str(index) + '/</D:href><D:propstat><D:prop>' + calendar_props + '</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>' for index in [1, 2])
                    payload = payload.replace(b'</D:multistatus>', responses.encode() + b'</D:multistatus>')
                if self.command == 'REPORT':
                    payload = b'<D:multistatus xmlns:D="DAV:"><D:sync-token>urn:fixture:complete</D:sync-token></D:multistatus>'
                if mode == 'invalid':
                    payload = b'<not-multistatus/>'
                self.send_response(207)
                self.end_headers()
                self.wfile.write(payload)
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            env = dict(os.environ, CALDAV_ORIGIN=f'http://127.0.0.1:{server.server_port}', CALDAV_USERNAME='fixture-user', CALDAV_PASSWORD='fixture-password-private')
            result = subprocess.run(['python3', str(SCRIPT)], env=env, capture_output=True, text=True, timeout=10)
        finally:
            server.shutdown()
            server.server_close()
            thread.join()
        self.assertNotIn('fixture-password-private', result.stdout + result.stderr)
        self.assertNotIn('Zml4dHVyZS11c2Vy', result.stdout + result.stderr)
        return result, requests

    def test_discovery_preserves_method_body_depth_and_auth(self):
        result, requests = self.run_fixture('valid')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([r[:2] for r in requests[:3]], [('GET', '/.well-known/caldav'), ('PROPFIND', '/.well-known/caldav'), ('PROPFIND', '/dav/')])
        self.assertEqual(requests[1][2], BODY)
        self.assertEqual(requests[1][2:], requests[2][2:])
        self.assertEqual(requests[2][3], '0')
        self.assertTrue(requests[2][4].startswith('Basic '))
        self.assertEqual([r[:2] for r in requests[-2:]], [('REPORT', '/dav/calendars/1/1/'), ('REPORT', '/dav/calendars/1/2/')])
        self.assertIn('(2 calendars)', result.stdout)

    def test_unsafe_redirect_is_rejected_before_following(self):
        result, requests = self.run_fixture('unsafe')
        self.assertEqual(result.returncode, 1)
        self.assertIn('leaves configured origin', result.stderr)
        self.assertEqual(len(requests), 1)

    def test_http_success_requires_xml_semantics(self):
        result, _ = self.run_fixture('invalid')
        self.assertEqual(result.returncode, 1)
        self.assertIn('root is not multistatus', result.stderr)


if __name__ == '__main__':
    unittest.main()
