# Portable installation lifecycle

These scripts manage the x64 MSVC portable ZIP produced by `package.ps1`. They
are PowerShell scripts for Windows administration only; they do not install a
service, write registry keys, create shortcuts, modify `PATH`, or claim that
the application is signed. No installer framework is used.

## Runtime data contract

The application keeps its data beside `taskbar-groups.exe` in these
executable-relative directories:

- `config`
- `Shortcuts`
- `JITComp`

The lifecycle scripts treat all three as user data. Upgrade preserves them,
and uninstall preserves them unless `-PurgeData` is explicitly supplied.
Unknown files and directories are also left alone by uninstall.

Close `taskbar-groups.exe` before upgrading or uninstalling. Windows file locks
can otherwise prevent a complete transaction.

## Install

Install requires a destination that does not already exist. This prevents an
accidental install from overwriting an existing portable directory.

```powershell
.\packaging\install-portable.ps1 `
  -Archive .\artifacts\taskbar-groups-v0.1.0-x86_64-pc-windows-msvc.zip `
  -InstallPath "$env:LOCALAPPDATA\TaskbarGroups"
```

Use `-DryRun` to validate the ZIP path/layout and show the planned operation
without creating directories or extracting files. The command also supports
PowerShell's standard `-WhatIf` and `-Confirm` safeguards.

## Upgrade

Upgrade stages and validates the new ZIP, moves the existing installation to a
same-volume temporary backup, installs the staged directory, then copies the
three protected data directories back. If any step fails, it removes the new
directory and restores the original from the backup. The backup is deleted
only after the operation succeeds.

```powershell
.\packaging\upgrade-portable.ps1 `
  -Archive .\artifacts\taskbar-groups-v0.1.1-x86_64-pc-windows-msvc.zip `
  -InstallPath "$env:LOCALAPPDATA\TaskbarGroups"
```

`-DryRun` validates the ZIP and reports the preservation behavior without
changing the installation. Use `-Confirm` for an interactive confirmation, or
`-WhatIf` for PowerShell's built-in simulation mode.

## Uninstall

The default uninstall removes only the known application files and retains
`config`, `Shortcuts`, `JITComp`, unknown files, and unknown directories. The
installation directory is removed only when it is empty afterward.

```powershell
.\packaging\uninstall-portable.ps1 `
  -InstallPath "$env:LOCALAPPDATA\TaskbarGroups"
```

To intentionally delete the three runtime data directories, use both the
explicit purge switch and confirmation:

```powershell
.\packaging\uninstall-portable.ps1 `
  -InstallPath "$env:LOCALAPPDATA\TaskbarGroups" `
  -PurgeData -Confirm
```

Review a purge with `-DryRun` first. These scripts do not remove an arbitrary
parent directory and do not delete unrecognized files.

## Validation and limitations

The scripts validate the ZIP extension, reject path traversal/absolute ZIP
entries, and require a root-level `taskbar-groups.exe`. `Expand-Archive` and
`.NET System.IO.Compression` are supplied by Windows PowerShell/.NET; no
additional package is required.

The scripts cannot determine whether the process is running, so the operator
must close the application first. Upgrade rollback depends on the install and
its parent being writable and on Windows allowing the directory moves; if
rollback itself fails, the error includes the temporary backup path. The
transaction is not a system-wide atomic installer transaction, and it does not
verify a cryptographic signature. Validate release integrity separately using
the existing packaging manifest/validator.
