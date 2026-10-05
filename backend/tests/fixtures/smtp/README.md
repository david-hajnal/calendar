These certificates and the published private key belong only to the controlled
loopback SMTP test server. Never use them for production. The server certificate
is signed by the fixture CA and covers `localhost`; tests explicitly trust that
CA to exercise successful TLS, and separately verify that the normal provider
rejects it. The CA private key is not stored in this repository.
