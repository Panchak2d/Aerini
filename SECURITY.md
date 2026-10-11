# Security Policy

## Reporting a vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Report security issues privately:

- **Email:** See [CONTACT.md](CONTACT.md)
- **GitHub Private Vulnerability Reporting:** enabled in repository settings

Include:
- Description of the vulnerability
- Steps to reproduce
- Affected versions
- Potential impact

---

## Response expectations

| Timeline | What happens |
|----------|--------------|
| 48 hours | Acknowledgment of your report |
| 7 days | Initial assessment and severity classification |
| 30 days | Fix or mitigation for confirmed vulnerabilities |
| 90 days | Public disclosure (coordinated with reporter) |

We follow responsible disclosure. We will not take legal action against
researchers who report in good faith.

---

## Supported versions

| Version | Security patches |
|---------|-----------------|
| latest release | ✅ Yes |
| previous minor | ✅ Yes (for 90 days after new minor release) |
| older versions | ❌ No |

---

## Scope

In scope: the Aerini engine, server binary, Tauri desktop app, and official Docker image.

Out of scope: third-party services integrated via nodes (Slack, Stripe, etc.).

## Known limitations

Postgres and MySQL connections are checked against the private-address block list only before they connect, so a DNS-rebinding attacker who controls a hostname's DNS answers may still redirect them to an internal address. The database library uses one host value both to dial and for TLS verification, so pinning the address would break certificate checks. Setting `sslmode=verify-full` (Postgres) or `ssl-mode=VERIFY_IDENTITY` (MySQL) in the connection URL makes the server prove it holds the configured name, so a rebound internal address fails the TLS handshake; the other modes, including the default, do not check the name. Every other outbound node, and `.wasm` plugin HTTP, checks the address again when it connects. If workflows from untrusted sources can run on a host, block outbound connections to private, loopback, link-local and cloud-metadata ranges with a host-level egress firewall. See [docs/guide/security.md](docs/guide/security.md#ssrf-protection).
