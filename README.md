# Taskbar Groups (Rust)

A Rust reimplementation of [Taskbar Groups](https://github.com/tjackenpacken/taskbar-groups), a Windows utility for organizing shortcuts into taskbar-launchable groups.

This repository is intentionally separate from the legacy implementation. The legacy C# source is kept in [`reference/`](reference/) for behavioral and compatibility research; it is not built by this project.

## Status

The project is at the architecture and discovery stage. The current binary is a dependency-free Rust smoke-test scaffold that establishes the application data directories and launch-mode boundary.

## Development

```text
cargo run
cargo run -- "Example Group"
cargo test
```

The first command uses configuration mode. Passing a group name exercises the group-launch path. Windows-specific behavior and the native UI will be implemented behind the modules in `src/platform` and `src/ui`.

## Design constraints

- Rust is the implementation language.
- The legacy repository remains a reference only.
- Domain behavior must not depend on UI or Windows APIs.
- Optional/custom dependencies will be evaluated as separate repositories/crates rather than being embedded into this application repository.
- Configuration format and migration behavior will be decided after issue and pull-request review.

## License

The rewrite is licensed under MIT. See [`LICENSE`](LICENSE). The reference project retains its original license and history in its nested repository.
