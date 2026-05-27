# Security Policy

## Supported Versions

Security fixes are handled for the latest `main` branch and the latest stable release tag.

| Version | Supported |
| --- | --- |
| 1.x | Yes |

## Reporting a Vulnerability

Please report suspected vulnerabilities privately by emailing `sunny.sudarshan@gmail.com`.

Include:

- A short description of the issue.
- Steps to reproduce or a minimal input sample.
- Expected and actual behavior.
- Any environment details that matter.

Do not open a public GitHub issue for secrets, exploit details, or private target data. I will review reports as soon as possible and coordinate a fix before public disclosure when needed.

## Secret Handling

This repository should not contain API keys, tokens, private certificates, signing keys, `.env` files, or local machine configuration. If a secret is committed by mistake, rotate it immediately and remove it from history only after deciding that cleanup is required.
