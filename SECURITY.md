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

Postgres and MySQL connections are checked before they connect, and the database library (sqlx 0.8) looks the host name up again when it dials, so a DNS-rebinding attacker who controls a hostname's DNS answers could otherwise redirect them to an internal address. Aerini closes this by dialling the address it validated whenever the TLS mode does not check the host name (every mode except `sslmode=verify-full` for Postgres and `ssl-mode=VERIFY_IDENTITY` for MySQL, including the default), so no TLS server name is sent in those modes. With `verify-full` or `VERIFY_IDENTITY` the host name has to stay a name for the certificate check, so the connection is not pinned: a rebound internal address fails the TLS handshake before credentials are sent, but the TCP connection attempt itself can still reach it. Block outbound connections to private, loopback, link-local and cloud-metadata ranges with a host-level egress firewall if workflows from untrusted sources can run on a host. Every other outbound node, and `.wasm` plugin HTTP, checks the address again when it connects. See [docs/guide/security.md](docs/guide/security.md#ssrf-protection).
