# Windows CI coverage

The `windows-compatibility` job validates the supported `x86_64-pc-windows-msvc`
target on the GitHub-hosted `windows-2022` and `windows-2025` images. Each matrix
leg runs locked cargo checks and tests, the Windows acceptance test target, the
portable package build, package validation, and the portable lifecycle test.
Package outputs are retained as CI artifacts for 14 days.

## Operating-system scope

The runner labels currently identify GitHub-hosted Windows Server images. They do
not guarantee a Windows 10 or Windows 11 client operating system:

- `windows-2022` is Windows Server 2022, not a Windows 10 client runner.
- `windows-2025` is Windows Server 2025, not a Windows 11 client runner.
- No Windows 10 or Windows 11 client label is assumed by this workflow.

Consequently, this matrix provides hosted Windows/MSVC and Windows API coverage,
not a claim that the application has been tested on both Windows 10 and Windows
11 client editions. Client-version coverage requires self-hosted runners or an
external validation service with those exact operating systems.

The acceptance tests are safe for non-interactive hosted agents: tests that need
a desktop, Explorer, COM, or another interactive shell facility report a clear
`SKIP` when that facility is unavailable. Non-interactive process and filesystem
coverage continues to run.
