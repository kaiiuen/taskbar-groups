# Windows release packaging

## Release contract

`release-metadata.json` is the checked-in release contract. The packager verifies
that its version matches `Cargo.toml`, records the exact source commit and Rust
compiler in the archive, and writes `artifacts/manifest.json` with the archive
SHA-256. The archive is a portable ZIP; it is not signed and no installer is
provided. These artifacts have reproducible version/source provenance, but the
ZIP itself is not promised to be byte-for-byte reproducible because ZIP entry
timestamps are supplied by the archiver.

The only implemented release target is `x86_64-pc-windows-msvc` on Windows.
ARM64 is intentionally listed as unsupported until a build and validation path
exists. Do not describe the artifact as signed or ARM64-capable.

## Build and validate

From the repository root on Windows with the MSVC Rust toolchain:

```powershell
cargo test --locked
```

```powershell
.\packaging\package.ps1
.\packaging\validate.ps1 -Archive .\artifacts\taskbar-groups-v0.1.0-x86_64-pc-windows-msvc.zip
```

Packaging uses `cargo build --locked --release` and creates:

- `artifacts/taskbar-groups-v<version>-x86_64-pc-windows-msvc.zip`
- `artifacts/manifest.json` (source commit, target, and SHA-256)

The ZIP contains `taskbar-groups.exe`, `LICENSE`,
`RELEASE-METADATA.json`, and empty `config/`, `Shortcuts/`, and `JITComp/`
directories. The executable and those directories are the complete runtime
layout; user data is not included.

## Portable and installed-style use

For a portable install, extract the ZIP to a user-writable directory and run
`taskbar-groups.exe` there. Configuration and generated shortcuts remain beside
the executable, so the directory must be writable.

An installed-style deployment is also a manual extraction to a chosen
installation directory (for example, a per-user applications directory). No
registry entries, services, shortcuts, or PATH changes are created by this
package. Keep the install directory writable, or use the portable layout.

To upgrade, close the application, extract the new ZIP over the existing
installation, and retain `config/`, `Shortcuts/`, and `JITComp/`. Back up those
directories before upgrading. To uninstall, close the application and remove
the installation directory; back up or remove its adjacent runtime data as
appropriate. The validator exercises clean extraction, preservation of a
configuration sentinel during upgrade, and complete removal of the install
folder during uninstall.

The workflow runs on pull requests and relevant pushes to `master`, on Windows
where the MSVC linker and Windows SDK are available. It runs locked tests and a
locked release build, requires exactly one expected x64 MSVC ZIP, then checks
version/target/provenance metadata, the manifest SHA-256, required runtime
layout, forbidden files, and clean install/upgrade/uninstall behavior. It also
checks that a tampered manifest is rejected. Validation requires only
PowerShell and .NET ZIP support.

There is no installer, signing step, or ARM64 package in this contract.
