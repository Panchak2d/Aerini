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
