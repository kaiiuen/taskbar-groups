# Windows packaging

`package.ps1` builds the locked `x86_64-pc-windows-msvc` release binary and creates a versioned portable ZIP under `artifacts/`.

The archive contains only the executable, `LICENSE`, and the runtime directories used beside the executable: `config/`, `Shortcuts/`, and `JITComp/`. The marker files keep those directories present in the ZIP; user data is created beside the executable at runtime and is not packaged.

Run the validation independently after packaging:

```powershell
.\packaging\validate.ps1 -Archive .\artifacts\taskbar-groups-v0.1.0-x86_64-pc-windows-msvc.zip
```

The workflow runs on Windows, where the MSVC linker and Windows SDK are available. Local packaging therefore requires a Windows MSVC Rust toolchain; validation only requires PowerShell/.NET ZIP support.
