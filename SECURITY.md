# Security Policy

We take security seriously at Local-Warp and appreciate the efforts of security researchers who help keep users safe.

## BYOK/BYOP Secret Handling

Local-Warp's BYOK/BYOP features let users provide their own API keys and provider endpoints. Treat these as sensitive:

- Do not paste raw provider API keys, bearer tokens, managed-secret values, or private endpoint credentials into GitHub issues, PRs, logs, screenshots, or documentation.
- Provider API keys are expected to live in the OS keychain through `AgentProviderSecrets`, not in tracked files.
- Test fixtures should use fake keys and local mock endpoints.
- Security reports involving provider-key leakage, incorrect endpoint routing, or Remote BYOP credential forwarding should be reported privately through the channels below.

## Reporting a Vulnerability

If you believe you've found a security vulnerability, please follow responsible disclosure practices and **do not** open a public GitHub issue or pull request, as this could expose the vulnerability before a fix is available.

Instead, please report it through one of the following channels:

- **Email:** [security@warp.dev](mailto:security@warp.dev)
- **GitHub Security Advisory:** [Open a private advisory](https://github.com/warpdotdev/Warp/security/advisories/new)

We will acknowledge your report promptly and work with you to understand and resolve the issue as quickly as possible.
