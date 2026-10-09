#!/usr/bin/env python3
"""Real OpenSSL certificates, mocked kubectl; never contacts a cluster."""
import base64
import json
import os
from pathlib import Path
import sys
import signal
import time
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class AuthTLS(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / 'bin').mkdir()
        (self.root / 'scratch').mkdir()
        (self.root / 'bin/python3').symlink_to(sys.executable)
        mock = self.root / 'bin/kubectl'
        mock.write_text('#!'+sys.executable+'''

import json,os,pathlib,stat,sys
root=pathlib.Path(os.environ['FIXTURE'])
with (root/'calls').open('a') as out: out.write(json.dumps(sys.argv[1:])+'\\n')
if sys.argv[1]=='get':
    if (root/'wait').exists():
        (root/'waiting').touch()
        import time
        time.sleep(20)
    if (root/'get-error').exists(): sys.exit(1)
    if (root/'secret').exists(): print((root/'secret').read_text())
elif sys.argv[1]=='create':
    if (root/'create-error').exists(): sys.exit(1)
    args=sys.argv[1:]
    cert=pathlib.Path(next(a.split('=',1)[1] for a in args if a.startswith('--cert=')))
    key=pathlib.Path(next(a.split('=',1)[1] for a in args if a.startswith('--key=')))
    assert stat.S_IMODE(key.stat().st_mode)==0o600
    assert stat.S_IMODE(key.parent.stat().st_mode)==0o700
    import base64
    (root/'created').write_text(json.dumps({'type':'kubernetes.io/tls','data':{
      'tls.crt':base64.b64encode(cert.read_bytes()).decode(),
      'tls.key':base64.b64encode(key.read_bytes()).decode()}}))
else: sys.exit('unexpected mutation')
''')
        mock.chmod(0o755)
        self.env = dict(os.environ, PATH=str(mock.parent)+os.pathsep+os.environ['PATH'],
                        FIXTURE=str(self.root), TMPDIR=str(self.root/'scratch'), KUBECONFIG='mock')

    def cert(self, host='auth.hajnal.space', days=365):
        cert, key = self.root/'cert.pem', self.root/'key.pem'
        subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-sha256',
                        '-days', str(days), '-nodes', '-subj', '/CN='+host,
                        '-addext', 'subjectAltName=DNS:'+host, '-out', str(cert), '-keyout', str(key)],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        return {'type': 'kubernetes.io/tls', 'data': {
            'tls.crt': base64.b64encode(cert.read_bytes()).decode(),
            'tls.key': base64.b64encode(key.read_bytes()).decode()}}

    def run_helper(self, success=True, validate=False, dry=False):
        command = ['bash', str(ROOT/'deploy/provision-auth-tls.sh')]
        if validate:
            command = ['bash', '-c', 'source "$1"; auth_tls_secret commoncal validate',
                       'test', str(ROOT/'deploy/auth-tls.sh')]
        env = dict(self.env, DRY_RUN='1' if dry else '0')
        result = subprocess.run(command, env=env, text=True, capture_output=True)
        self.assertEqual(result.returncode == 0, success, result.stdout+result.stderr)
        self.assertEqual(list((self.root/'scratch').iterdir()), [], 'TLS scratch leaked')
        self.assertNotIn('PRIVATE KEY', result.stdout+result.stderr)
        calls = (self.root/'calls').read_text()
        self.assertNotIn('PRIVATE KEY', calls)
        self.assertNotIn('--from-literal', calls)
        if not success:
            self.assertFalse((self.root/'created').exists())
        return result

    def existing(self, secret):
        (self.root/'secret').write_text(json.dumps(secret))

    def test_creation_and_reuse(self):
        self.run_helper()
        created = json.loads((self.root/'created').read_text())
        cert = base64.b64decode(created['data']['tls.crt'])
        info = subprocess.run(['openssl', 'x509', '-noout', '-text'], input=cert,
                              capture_output=True, check=True).stdout.decode()
        self.assertIn('DNS:auth.hajnal.space', info)
        self.assertIn('sha256WithRSAEncryption', info)
        self.assertIn('2048 bit', info)
        self.existing(created)
        (self.root/'created').unlink()
        self.run_helper()
        self.assertFalse((self.root/'created').exists())
        self.run_helper(validate=True)

    def test_wrong_type(self):
        secret = self.cert(); secret['type'] = 'Opaque'; self.existing(secret)
        self.assertIn('not overwritten', self.run_helper(False).stderr)

    def test_wrong_host(self):
        self.existing(self.cert('cal.hajnal.space'))
        self.assertIn('does not cover', self.run_helper(False).stderr)

    def test_mismatched_key(self):
        secret = self.cert(); secret['data']['tls.key'] = self.cert()['data']['tls.key']
        self.existing(secret)
        self.assertIn('do not match', self.run_helper(False).stderr)

    def test_malformed_certificate(self):
        secret = self.cert(); secret['data']['tls.crt'] = base64.b64encode(b'bad').decode()
        self.existing(secret); self.run_helper(False)

    def test_missing_key(self):
        secret = self.cert(); del secret['data']['tls.key']
        self.existing(secret); self.run_helper(False)

    def test_bad_base64(self):
        secret = self.cert(); secret['data']['tls.key'] = '!!!'
        self.existing(secret); self.run_helper(False)

    def test_expiring(self):
        self.existing(self.cert(days=29))
        self.assertIn('30 days', self.run_helper(False).stderr)

    def test_expired(self):
        secret = self.cert()
        subprocess.run(['openssl', 'x509', '-in', str(self.root/'cert.pem'), '-signkey',
                        str(self.root/'key.pem'), '-days', '0', '-out', str(self.root/'expired.pem')],
                       check=True, capture_output=True)
        secret['data']['tls.crt'] = base64.b64encode((self.root/'expired.pem').read_bytes()).decode()
        self.existing(secret); self.run_helper(False)

    def test_read_error_does_not_create(self):
        (self.root/'get-error').touch(); self.run_helper(False)

    def test_generation_failure_cleanup(self):
        wrapper = self.root/'bin/openssl'
        wrapper.write_text('#!/bin/sh\nexit 1\n')
        wrapper.chmod(0o755)
        self.run_helper(False)

    def test_create_failure_cleanup(self):
        (self.root/'create-error').touch(); self.run_helper(False)

    def test_read_only_missing(self):
        self.run_helper(False, validate=True)

    def test_dry_run(self):
        self.run_helper(dry=True)
        self.assertIn('--dry-run=server', (self.root/'calls').read_text())

    def test_signal_cleanup(self):
        (self.root/'wait').touch()
        proc = subprocess.Popen(['bash', str(ROOT/'deploy/provision-auth-tls.sh')],
                                env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                start_new_session=True)
        try:
            deadline = time.monotonic()+5
            while not (self.root/'waiting').exists() and time.monotonic()<deadline:
                time.sleep(0.02)
            self.assertTrue((self.root/'waiting').exists())
            os.killpg(proc.pid, signal.SIGTERM)
            proc.communicate(timeout=5)
            self.assertNotEqual(proc.returncode, 0)
            self.assertEqual(list((self.root/'scratch').iterdir()), [])
        finally:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.communicate()

    def test_integration(self):
        for filename in ('bootstrap-auth-sqlite.sh', 'bootstrap-production.sh', 'deploy-prod.sh'):
            text = (ROOT/'deploy'/filename).read_text()
            self.assertIn('/auth-tls.sh"', text)
            self.assertIn(' provision', text)
        self.assertIn(' validate', (ROOT/'deploy/auth-prerequisites.sh').read_text())


if __name__ == '__main__':
    unittest.main()
