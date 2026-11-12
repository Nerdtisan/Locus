# Packaging and installers

Spec: [SPEC §0](SPEC.md) (Packaging), [§13](SPEC.md) M4. Epic #29.

## How builds are produced

`.github/workflows/release.yml` builds every installer. It runs on **manual dispatch**
(Actions > release > Run workflow) and on **`v*` tags**, and also on pull requests that touch
`packaging/` or the workflow itself, so packaging changes are proven before merging.

Builds are **private**: installers are uploaded as workflow artefacts (14-day retention). No
GitHub Release is created and nothing is submitted to Flathub. A `SHA256SUMS-<version>`
artefact lists the hash of every installer.

The version comes from the tag (`v1.2.3` -> `1.2.3`), otherwise from `Cargo.toml` plus the short
commit (`0.0.0-dev.abc1234`).

## Signing (not yet)

All installers are **unsigned**. Each platform has a signing step in the workflow that is skipped
until its secret exists. To enable signing later, add the secrets in
Settings > Secrets and variables > Actions; never commit certificates or passwords.

| Platform | Secrets the stub will look for |
|---|---|
| Windows (Authenticode) | `WINDOWS_CERT_PFX_BASE64`, `WINDOWS_CERT_PASSWORD` |
| macOS (Developer ID + notarisation) | `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD`, `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` |

Until then, expect the warnings below.

## Pinned tools

Every packaging tool is pinned to an exact version (and a SHA-256 where it is downloaded); bump
them deliberately, not by floating tags.
