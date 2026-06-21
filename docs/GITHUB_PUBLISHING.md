# GitHub publishing checklist

## Suggested repository settings

- Repository name: `codex-migrate`
- Description: `Cross-platform GUI and CLI for migrating, repairing, backing up, and exporting local Codex sessions.`
- Website: leave blank unless a documentation site is created.
- License: MIT
- Default branch: `main`
- Suggested topics:
  - `codex`
  - `migration`
  - `backup`
  - `rust`
  - `egui`
  - `sqlite`
  - `cross-platform`
  - `desktop-app`
  - `session-manager`

## Initial publication

```bash
cd codex-migrate
git init
git branch -M main
git add .
git commit -m "Initial open-source release"
git remote add origin https://github.com/ChenglongLi777/codex-migrate.git
git push -u origin main
```

Before pushing:

1. Review the public author/contact information.
2. Confirm that `git status --ignored` shows `target/`, `dist/`, and local environment files as ignored.
3. Run the validation commands in `CONTRIBUTING.md`.
4. Enable GitHub Issues and Private Vulnerability Reporting.
5. Add a social preview image in repository settings if desired.

## First release

The release workflow starts when a tag beginning with `v` is pushed:

```bash
git tag v1.0.7
git push origin v1.0.7
```

The tag version must match the version in `Cargo.toml`. GitHub Actions will build platform archives, generate SHA-256 checksum files, and attach them to a GitHub Release.

Unsigned builds may trigger Windows SmartScreen or macOS Gatekeeper. Do not describe unsigned binaries as trusted or verified by the operating-system vendor.

## Recommended branch rules

Protect `main` and require:

- pull requests before merging;
- the `CI` status check;
- conversation resolution;
- no force pushes.

## Branding

Keep the independent-project disclaimer visible in the README and application distribution. Do not use the OpenAI logo or imply endorsement.
