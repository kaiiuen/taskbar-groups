//! Temporary UI adapter. A Windows window, dialogs, and shell integration can
//! replace this without changing the application controller.

use std::io;

use crate::platform::LaunchSpec;

pub trait UiShell {
    fn show_message(&mut self, message: &str);
    fn show_error(&mut self, message: &str);
    fn launch(&mut self, spec: &LaunchSpec) -> Result<(), io::Error>;
    fn reveal_group(&mut self, shortcut_path: &str);
}

#[derive(Debug, Default)]
pub struct TemporaryShell;

impl UiShell for TemporaryShell {
    fn show_message(&mut self, message: &str) {
        println!("{message}");
    }

    fn show_error(&mut self, message: &str) {
        eprintln!("{message}");
    }

    fn launch(&mut self, spec: &LaunchSpec) -> Result<(), io::Error> {
        println!("temporary launch adapter: {spec:?}");
        Ok(())
    }

    fn reveal_group(&mut self, shortcut_path: &str) {
        println!("temporary shell adapter: reveal {shortcut_path}");
    }
}
