//! This is a debug GUI for the terminal emulator backend I am building for
//! Sesh. The terminal emulator will be included in this crate, but the GUI
//! stuff is all temporary and for debugging only. The library will be used
//! in sesh to multiplex terminal sessions and allow multiple applications to
//! run in the same terminal window. Currently sesh works by piping the output directly
//! from the server to the client which is very limiting, but this will allow for scrollback,
//! multiple panes, and proper keymappings.
use std::{
    ffi::{CStr, CString},
    io::{BufReader, Read},
    ops::Deref,
    os::fd::{FromRawFd, OwnedFd},
    pin::Pin,
    process::Stdio,
    sync::{Arc, RwLock},
    task::Context,
};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt},
    sync::Mutex,
};

use anyhow::Result;
use eframe;
use egui;
use nix::pty::ForkptyResult;

mod pty;

pub struct TermGui {
    buffer: Arc<tokio::sync::RwLock<Vec<u8>>>,
    file: Arc<Mutex<std::fs::File>>,
}

impl TermGui {
    fn new(_cc: &eframe::CreationContext<'_>, fd: OwnedFd) -> Self {
        Self {
            file: Arc::new(Mutex::new(std::fs::File::from(fd))),
            buffer: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        }
    }

    fn run(&mut self) {
        std::thread::spawn({
            let buffer = self.buffer.clone();
            let file = self.file.clone();
            move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to build tokio runtime");
                let mut buf = vec![0; 4096];
                rt.block_on(async move {
                    loop {
                        match file.lock().await.read(&mut buf) {
                            Ok(bytes) => {
                                buffer.write().await.extend_from_slice(&buf[..bytes]);
                            }
                            Err(_) => {
                                break;
                            }
                        }
                    }
                });
            }
        });
    }
}

impl eframe::App for TermGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(unsafe { std::str::from_utf8_unchecked(&self.buffer.blocking_read()) })
        });
    }
}

fn main() -> Result<()> {
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
            let mut app = TermGui::new(cc, fd);
            app.run();
            Box::new(app)
        }),
    )
    .map_err(|e| anyhow::anyhow!("{}", e))
}
