# Security Policy

**中文：** [SECURITY.zh-CN.md](SECURITY.zh-CN.md)

## Supported versions

| Version | Supported |
|---------|-----------|
| `main` (0.1.x) | Yes |
| Older tags | Best effort |

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security vulnerabilities.

1. Email: **lanpishu6300@gmail.com** with subject `[SECURITY] match-rust`
2. Or use GitHub **Security Advisories** on [lanpishu6300/match-rust](https://github.com/lanpishu6300/match-rust/security/advisories/new) if available

Include:

- Affected crate / component
- Reproduction steps or PoC (private)
- Impact assessment (auth bypass, DoS, data leak, etc.)

We aim to acknowledge within **72 hours** and provide a remediation plan or fix timeline.

## Scope notes

- Matching engines process untrusted order JSON in production paths — validation bugs are in scope.
- Experimental crates (`match-core-hp`, `match-wal`) are still in scope if they can be enabled in a shell.
- Dependency CVEs: prefer PRs bumping versions with a short risk note.
