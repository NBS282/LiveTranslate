# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| latest  | ✅                 |
| older   | ❌                 |

## Reporting a Vulnerability

LiveTranslate is a local-only application — **no audio, text, or personal data ever leaves your machine**. However, if you discover a security vulnerability:

1. **Do NOT open a public issue**
2. Email the maintainer directly (or reach out via GitHub)
3. Include a description of the issue, steps to reproduce, and impact assessment

We aim to respond within 72 hours and patch critical issues within a week.

## What We Care About

- Remote code execution via crafted audio/model files
- Privilege escalation
- Data exfiltration from the engine sandbox
- Supply chain attacks on dependencies

## What We DON'T Worry About

- AI model hallucination or translation quality (that's a feature, not a security issue)
- Telemetry or analytics (there is none)
- API keys (there are none — everything runs locally)
