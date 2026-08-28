# Reverse-engineering notes

## Reference snapshot

- Repository: `tjackenpacken/taskbar-groups`
- Default branch: `master`
- Snapshot cloned under `reference/`
- Latest repository commit at initial review: `edbd4d9` (2021-01-05)
- License: MIT
- Original implementation: C# WinForms

## Domain/config findings

### `Category`

`reference/main/Classes/Category.cs` is public-field XML-serializable and has these exact legacy element names:

- `Name`: group name; no initializer. The UI requires a non-empty value and accepts only ASCII letters, digits, and spaces in its intended validation (the C# regex uses `IsMatch` without anchoring, so invalid suffixes can accidentally pass).
- `ColorString`: defaults to `#1F1F1F` from `ColorTranslator.ToHtml(Color.FromArgb(31, 31, 31))`. Older files can omit it; the edit form repairs a null value to this default.
- `allowOpenAll`: defaults to `false`.
- `ShortcutList`: `List<ProgramShortcut>`; the parameterless constructor leaves it null, while the new-group UI explicitly initializes an empty list.
- `Width`: `int`, default CLR value `0`; the UI writes the selected integer but does not validate its range in the save path.
- `Opacity`: `double`, defaults to `10`; the UI writes this value, but the save path does not independently validate its range.

The UI refuses to save a group with no shortcuts, limits a group to 20 shortcuts, and requires an icon. Icon presence is a filesystem/UI concern and is intentionally not part of the pure Rust domain model. Group names are converted from spaces to underscores only when the config directory is created; the serialized `Name` is therefore normally underscore-separated.

### `ProgramShortcut`

`reference/main/Classes/ProgramShortcut.cs` uses these exact XML element names:

- `FilePath` (`string` property): required by normal UI flow, with no constructor default.
- `isWindowsApp` (`bool` property): defaults to `false`.
- `name` (`string` property): defaults to an empty string.
- `Arguments` (`string` public field): defaults to an empty string.
- `WorkingDirectory` (`string` public field): initialized from `MainPath.exeString` at object construction. This is the executable path, not necessarily the target's directory; edited entries may be repaired to the target directory before save.

Both classes have public parameterless constructors because .NET `XmlSerializer` requires them. Missing serialized members retain constructor/field initializer defaults during deserialization. The Rust model uses owned strings and an empty collection for safe operation, so importing a legacy file with missing `ShortcutList` differs from the C# null state but matches the UI's expected behavior.

### XML shape and compatibility risks

`XmlSerializer(typeof(Category))` writes a `Category` root, scalar elements named exactly as above, then a `ShortcutList` containing `ProgramShortcut` elements. There is no schema version, namespace contract, or explicit migration marker. Rust currently emits/reads this narrow element shape with std-only escaping; it is not a general XML parser and should be replaced or extended before accepting arbitrary third-party XML.

The legacy save validation is UI-only and incomplete: its name regex is not anchored, `Width`/`Opacity` are not range-checked, and icon/file existence checks happen elsewhere. The legacy default for `WorkingDirectory` depends on process startup state, and paths/config directories are derived beside the executable. These are the main compatibility risks for import and future persistence work.

## Persistence boundary

The Rust persistence boundary keeps the legacy portable layout beside the executable:
`JITComp/`, `config/`, and `Shortcuts/` are created at startup. A group is stored
under `config/<stored-name>/ObjectData.xml`, with `GroupImage.png`, `GroupIcon.ico`,
and an `Icons/` cache reserved alongside it. Group names are normalized by collapsing
whitespace to underscores for the config directory; the corresponding legacy taskbar
shortcut name changes underscores back to spaces and appends `.lnk`.

`AppPaths` accepts an explicit root for tests and portable deployments, while
`beside_executable` preserves the legacy executable-relative behavior. XML load/save
uses the domain's narrow `XmlSerializer`-compatible shape and reports malformed or
invalid documents as `io::ErrorKind::InvalidData`. Image generation, icon caching,
and Windows shortcut creation remain outside this persistence-only component.

## Observed behavior

1. Startup creates `JITComp`, `config`, and `Shortcuts` beside the executable.
2. A command-line argument selects group-launch mode; no argument opens the group configuration UI.
3. Groups are stored as `config/<group>/ObjectData.xml`.
4. Each group also stores `GroupImage.png`, `GroupIcon.ico`, and an `Icons/` cache.
5. Group shortcuts are Windows `.lnk` files in `Shortcuts/` and carry a distinct AppUserModelID.
6. Entries may target executables, folders, `.lnk` files, Windows Store apps, and Steam shortcuts.
7. Launching supports number-key selection and optional Ctrl+Enter open-all behavior.
8. Shortcut entries include a display name, arguments, and working directory.
9. The UI exposes group columns/width, background color, and opacity.

## UI flow and orchestration findings

The WinForms surface has two entry flows. `client.Main` uses the first argument
only: no argument opens `frmClient` (configuration mode), while a group argument
loads `frmMain` (group-launch mode). The Rust UI controller preserves this mode
split and leaves window creation, dialogs, icon extraction, process launching,
and Explorer integration behind a shell adapter.

### Configuration mode

`frmClient.Reload` enumerates `config` directories, loads each `Category`, and
renders a category panel. A category panel displays the stored group name with
underscores shown as spaces, cached shortcut icons, and an edit/open-folder
interaction. The empty/non-empty list changes the help text. Add opens a new
`frmGroup`; edit opens the same editor with the loaded category. Any group load
failure is shown to the user while the remaining groups can still be listed.

`frmGroup` supports these state transitions:

- create a default group or edit an existing group;
- add executable, folder, `.lnk`, `.url`, or Windows-app targets (including
  multi-select/drop input), up to 20 shortcuts;
- select one shortcut and edit its display name, arguments, and working
  directory; remove it or move it up/down;
- choose dark, light, or custom color, width, opacity, and the open-all option;
- select or drop a group icon;
- save, cancel, or delete, followed by a configuration reload.

Save errors are field-level in the legacy UI: name is required and restricted
by the intended ASCII name rule, an icon and at least one shortcut are required,
and the shortcut count is capped at 20. Persistence and shell failures are
reported as messages. Editing an existing group removes/recreates its old
configuration and taskbar link as part of the save operation; the Rust
controller delegates the filesystem portion to persistence and keeps this
orchestration independent of a GUI.

### Group-launch mode

`frmMain` loads one category and closes when deactivated. Number keys `1`–`9`
select entries 1–9 and `0` selects entry 10; missing entries are ignored. On
Ctrl+Enter, all shortcuts are launched only when `allowOpenAll` is enabled.
Normal targets use path, arguments, and working directory; Windows apps use an
AppUserModelID activation path. The Rust controller turns these interactions
into launch plans and sends them to a temporary adapter, so tests never start a
process. Resolver, launcher, icon, taskbar placement, and Windows shell errors
remain explicit adapter concerns.

The dependency-free UI boundary is `src/ui/mod.rs`: `Action` expresses user
intent, `View` is renderable state, `Controller` coordinates domain/persistence/
platform contracts, and `UiShell` is the temporary replaceable side-effect
adapter. A native Windows frontend can later map controls and window events to
these actions without moving flow logic into event handlers.

## Risks to resolve before compatibility work

- The old XML schema is implicit in public C# fields and has no versioning strategy.
- Paths are frequently derived relative to the executable, which is fragile for installed and portable deployments.
- Icon cache naming is path-derived and can collide or become stale when targets move.
- Shell link/AppUserModelID behavior must be validated against current supported Windows versions.
- Existing issue and pull-request history must be triaged into compatibility requirements, bugs, and obsolete requests rather than copied blindly.

## Platform and launch boundary findings

`reference/main/client.cs` uses only the first argument after the executable. No
argument opens `frmClient`; an argument opens `frmMain` for that group. The
process AppUserModelID is `tjackenpacken.taskbarGroup.main` in configuration
mode, or `tjackenpacken.taskbarGroup.menu.<group>` in group mode. The legacy
startup also creates `JITComp`, `config`, and `Shortcuts` beside the executable.

`frmMain` maps number keys `1` through `9` to zero-based entries `0` through
`8`, and `0` to entry `9`. Bounds failures are swallowed, so a key with no
corresponding shortcut has no effect. On key-up, `Ctrl+Enter` opens every
shortcut only when `Category.allowOpenAll` is true; otherwise it has no launch
effect. Number-key launch and open-all are represented as a pure launch plan in
Rust so tests do not start child processes.

Normal entries are launched with the stored file path, arguments, and working
directory (`ProcessStartInfo`). Windows-app entries are activated through
`shell:appsFolder\\<FilePath>`, where the stored path is an application user
model ID. Legacy `.lnk` entries are passed to the shell/process layer rather
than being resolved by the form. `ShellLink.InstallShortcut` creates a COM
`.lnk`, sets description, working directory, arguments, icon, and the
`System.AppUserModel.ID` property before saving. The Rust boundary exposes
resolver and launcher traits; COM/WinRT activation and ShellLink creation are
Windows-only implementation work and are intentionally stubbed until an ABI
strategy is selected.

`handleFolder` obtains a large folder icon using `SHGetFileInfo` and destroys
the returned HICON after cloning it. `handleWindowsApp` resolves a shortcut to
an AUMID, finds the matching package through `PackageManager`, and reads the
manifest/logo; package discovery can fail through access restrictions. These
icon and package metadata concerns are not part of launch planning.

## Proposed stages

1. Inventory issues, pull requests, releases, and the effective user-facing contract.
2. Define a versioned Rust configuration model and migration/import strategy for legacy XML.
3. Implement pure domain validation and launch planning with tests on every platform.
4. Implement Windows shell-link, shortcut resolution, icon extraction/cache, and app discovery adapters.
5. Build the UI and accessibility/error-reporting flows around those stable contracts.
6. Add packaging, upgrade behavior, crash diagnostics, and end-to-end Windows tests.
