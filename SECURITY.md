# Security Policy

## Supported versions

Security fixes are applied to the latest released version.

## Reporting a vulnerability

Do not publish vulnerabilities that could expose local conversations, files, credentials, or unsafe path handling in a public issue.

Use GitHub Private Vulnerability Reporting if it is enabled for the repository. Otherwise, contact the maintainer privately through the contact method listed on the maintainer’s GitHub profile.

Include:

- affected version and operating system;
- minimal reproduction steps;
- expected and actual behavior;
- a redacted sample if required.

Do not include:

- `auth.json`;
- API keys or tokens;
- an entire `.codex` directory;
- real conversation rollouts;
- unredacted database files;
- exported HTML containing private messages or images.

## Local-data model

Codex Migrate operates on local files and does not intentionally upload migration data. Release binaries should be obtained from the project’s GitHub Releases page and verified against published SHA-256 checksums.

