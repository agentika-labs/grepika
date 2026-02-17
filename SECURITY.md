# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

**Please do NOT open a public GitHub issue for security vulnerabilities.**

Instead, please report them responsibly:

1. **GitHub**: Use [GitHub Security Advisories](https://github.com/agentika-labs/grepika/security/advisories/new)

### What to Include

- Description of the vulnerability
- Steps to reproduce
- Impact assessment
- Suggested fix (if any)

### Response Timeline

- **Acknowledgment**: Within 48 hours
- **Assessment**: Within 1 week
- **Fix**: Within 2 weeks for critical issues

## Security Architecture

Grepika implements defense-in-depth security for code search:

### Sandboxing Layers
1. **Workspace isolation** — All file operations confined to workspace root
2. **Path traversal blocking** — `..` sequences, null bytes, and absolute paths rejected
3. **Sensitive file detection** — 60+ patterns blocked (`.env`, `.ssh`, credentials, cloud configs, etc.)
4. **ReDoS protection** — Regex pattern length limits, nesting depth caps, and nested quantifier detection
5. **Response truncation** — 512KB cap prevents context exhaustion

### What We Protect Against
- Path traversal attacks (`../../../etc/passwd`)
- Sensitive file disclosure (`.env`, API keys, SSH keys, cloud credentials)
- ReDoS via crafted regex patterns
- Workspace escape via symlinks or absolute paths
- Context exhaustion via oversized responses

## Security Testing

All security mechanisms are covered by automated tests:

```bash
cargo test -- security
```

