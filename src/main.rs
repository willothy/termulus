//! This is a debug GUI for the terminal emulator backend I am building for
//! Sesh. The terminal emulator will be included in this crate, but the GUI
//! stuff is all temporary and for debugging only. The library will be used
//! in sesh to multiplex terminal sessions and allow multiple applications to
//! run in the same terminal window. Currently sesh works by piping the output directly
//! from the server to the client which is very limiting, but this will allow for scrollback,
//! multiple panes, and proper keymappings.
use std::ffi::CStr;

use anyhow::Result;
use eframe;
use gui::TermGui;
use nix::pty::ForkptyResult;

mod gui;
mod parser;
mod terminal;

fn main() -> Result<()> {
    // Temporary: sesh already contains the logic for handling process creation
    // and management. This is just for testing the terminal emulator.
    let ForkptyResult {
        master,
        fork_result,
    } = unsafe { nix::pty::forkpty(None, None).unwrap() };
    let fd = match fork_result {
        nix::unistd::ForkResult::Parent { .. } => master,
        nix::unistd::ForkResult::Child => {
            nix::unistd::execvp::<&CStr>(
                CStr::from_bytes_with_nul(b"ash\0")?,
                &[
                    CStr::from_bytes_with_nul(b"ash\0")?,
                    CStr::from_bytes_with_nul(b"--noprofile\0").unwrap(),
                    CStr::from_bytes_with_nul(b"--norc\0").unwrap(),
                ],
            )
            .unwrap();
            return Ok(());
        }
    };

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Debug GUI",
        native_options,
        Box::new(|cc| {
            let app = TermGui::new(cc, fd);
            Box::new(app)
        }),
    )
    .map_err(|e| anyhow::anyhow!("{}", e))
}
