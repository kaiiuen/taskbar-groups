pub mod assets;
pub mod domain;
pub mod persistence;
pub mod platform;
pub mod ui;

use std::io;

/// Run the dependency-free application shell.
///
/// The launch request selects configuration or group-launch mode, persistence
/// owns the portable filesystem layout, and the UI boundary owns orchestration
/// until native window and Windows platform adapters are available.
pub fn run() -> io::Result<()> {
    let paths = persistence::AppPaths::beside_executable()?;
    paths.ensure_directories()?;

    let request = platform::LaunchRequest::from_environment();
    ui::run(request, paths)
}
