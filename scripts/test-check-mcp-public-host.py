#!/usr/bin/env python3
"""Offline tests for the read-only production regression checker."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("checker", Path(__file__).with_name("check-mcp-public-host.py"))
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class PublicHostCheckTests(unittest.TestCase):
    def run_check(self, authenticated_status=200, unexpected_status=403):
        calls = []

        def response(message, token=None, host=checker.HOST, session=None, protocol=None):
            calls.append((message["method"], token, host, session))
            if token != "private-test-token":
                return 401, {"WWW-Authenticate": f'Bearer resource_metadata="{checker.METADATA}"'}, b""
            if host not in (checker.HOST, checker.HOST + ":443"):
                return unexpected_status, {}, b""
            if authenticated_status != 200:
                return authenticated_status, {}, b"Forbidden: Host header is not allowed"
            if message["method"] == "notifications/initialized":
                return 202, {}, b""
            result = ({"protocolVersion": "2025-03-26"} if message["method"] == "initialize"
                      else {"tools": []})
            return 200, {"Mcp-Session-Id": "test-session"}, json.dumps({
                "jsonrpc": "2.0", "id": message["id"], "result": result}).encode()

        with patch.object(checker, "private_token", return_value="private-test-token"), \
                patch.object(checker, "request", side_effect=response), \
                contextlib.redirect_stdout(io.StringIO()):
            result = checker.check(SimpleNamespace())
        return result, calls

    def test_complete_check_only_initializes_and_lists(self):
        result, calls = self.run_check()
        self.assertEqual(result, 0)
        self.assertEqual({call[0] for call in calls},
                         {"initialize", "notifications/initialized", "tools/list"})
        listings = [call for call in calls if call[0] == "tools/list" and call[1] == "private-test-token"]
        self.assertEqual({call[2] for call in listings}, {checker.HOST, checker.HOST + ":443"})
        self.assertTrue(all(call[3] == "test-session" for call in listings))

    def test_original_authenticated_host_failure_fails_check(self):
        with self.assertRaises(ValueError):
            self.run_check(authenticated_status=403)

    def test_unexpected_host_success_fails_check(self):
        with self.assertRaises(ValueError):
            self.run_check(unexpected_status=200)

    def test_sse_result_parser(self):
        body = b'event: message\r\ndata: {"jsonrpc":"2.0","id":2,"result":{"tools":[]}}\r\n\r\n'
        self.assertEqual(checker.rpc_result(200, body, 2), {"tools": []})

    def test_rpc_error_is_not_success(self):
        with self.assertRaises(ValueError):
            checker.rpc_result(200, b'{"jsonrpc":"2.0","id":2,"error":{"code":-32600}}', 2)


if __name__ == "__main__":
    unittest.main()
