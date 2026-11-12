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

## Windows (#245)

Artefacts: `Locus-<version>-windows-setup.exe` (Inno Setup 7.1.1) and
`Locus-<version>-windows-portable.zip` (`locus.exe`, `locus-cli.exe`, licence, notices).

The installer is **per-user** (no admin rights; installs to `%LOCALAPPDATA%\Programs\Locus`),
adds a Start menu entry, registers the `.locus` and `.locusplugin` file types under
`HKCU\Software\Classes`, and has an uninstaller (Settings > Apps) that removes the files and the
registry keys. Double-clicking a `.locus` file runs `locus.exe "<file>"`, which opens it.
`.locusplugin` is registered so Windows recognises the type; the app does not yet install a plugin
from a double-click (use Plugins > Install from file).

CI installs, associates, runs `locus-cli.exe`, uninstalls and checks nothing is left behind.
The script is `packaging/windows/locus.iss`; the stable `AppId` GUID must never change.

### SmartScreen warning (unsigned)

Until the binaries are signed, Windows shows **"Windows protected your PC"** the first time you run
the installer or `locus.exe`: click **More info**, then **Run anyway**. A browser-downloaded file
can be unblocked beforehand with right-click > Properties > **Unblock**. The warning fades as the
file gains reputation, and disappears with an Authenticode certificate (see Signing).

## macOS (#246)

Artefact: `Locus-<version>-macos-universal.dmg` containing `Locus.app` (arm64 + x86_64, built
natively on a macOS runner and joined with `lipo`), an `Applications` shortcut and a short
"READ ME FIRST". The bundle has an icon, an `Info.plist` declaring the `.locus` document type
(UTI `io.github.nerdtisan.locus.document`) and `.locusplugin`, and carries `locus-cli` in
`Contents/Resources`. It is ad-hoc signed (required for arm64 to run) but **not** Developer-ID
signed or notarised; both steps are stubbed in the workflow for a future Apple account
(`SIGN_IDENTITY` is honoured by `packaging/macos/build-dmg.sh`).

CI mounts the dmg, checks both architectures and the signature, runs `locus-cli`, and launches the
GUI binary for 10 seconds.

Known limitation: Finder delivers a double-clicked document as an Apple Event, not a command-line
argument, and the GUI toolkit does not forward that yet. So double-clicking a `.locus` launches
Locus but does not open the file; use File > Open. (The command line `locus file.locus` works.)

### Gatekeeper warning (unsigned)

macOS says Locus "cannot be opened because Apple cannot check it for malicious software". Once:
drag Locus to Applications, Control-click it > **Open** > **Open**; or System Settings >
Privacy & Security > **Open Anyway**; or `xattr -dr com.apple.quarantine /Applications/Locus.app`.
