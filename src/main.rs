//! This is a debug GUI for the terminal emulator backend I am building for
//! Sesh. The terminal emulator will be included in this crate, but the GUI
//! stuff is all temporary and for debugging only. The library will be used
//! in sesh to multiplex terminal sessions and allow multiple applications to
//! run in the same terminal window. Currently sesh works by piping the output directly
//! from the server to the client which is very limiting, but this will allow for scrollback,
//! multiple panes, and proper keymappings.
use std::{
    ffi::CStr,
    os::fd::{AsRawFd, OwnedFd},
};

use anyhow::Result;
use eframe;
use egui::{self, TextStyle, Vec2};
use nix::{
    errno::Errno,
    fcntl::{FcntlArg, OFlag},
    libc::O_ACCMODE,
    pty::ForkptyResult,
};

#[derive(Debug, Clone)]
struct CursorPos {
    x: usize,
    y: usize,
}

impl CursorPos {
    fn new(x: usize, y: usize) -> Self {
        Self { x, y }
    }
}

pub struct TermGui {
    buffer: Vec<u8>,
    cursor: CursorPos,
    char_size: Option<Vec2>,
    fd: OwnedFd,
}

fn get_char_size(ctx: &egui::Context) -> Vec2 {
    let font_id = ctx.style().text_styles[&egui::TextStyle::Monospace].clone();
    ctx.fonts(|fonts| {
        let height = font_id.size;
        let layout = fonts.layout(
            "@".to_string(),
            font_id,
            egui::Color32::default(),
            f32::INFINITY,
        );

        Vec2::new(layout.mesh_bounds.width(), height)
    })
}

impl TermGui {
    fn new(cc: &eframe::CreationContext<'_>, fd: OwnedFd) -> Self {
        cc.egui_ctx.style_mut(|style| {
            style.override_text_style = Some(TextStyle::Monospace);
        });
        let flags = nix::fcntl::fcntl(fd.as_raw_fd(), FcntlArg::F_GETFL).expect("fcntl");
        let mut flags = OFlag::from_bits(flags & O_ACCMODE).unwrap();
        // set fd to nonblocking
        flags.set(OFlag::O_NONBLOCK, true);
        nix::fcntl::fcntl(fd.as_raw_fd(), FcntlArg::F_SETFL(flags)).expect("fcntl");
        Self {
            fd,
            cursor: CursorPos::new(0, 0),
            char_size: None,
            buffer: Vec::new(),
        }
    }

    fn char_to_cursor_offset(&self, char_size: &Vec2) -> Vec2 {
        let lines = self.buffer.split(|b| *b == b'\n').collect::<Vec<_>>();

        let x_off = self.cursor.x as f32 * char_size.x;
        let y_off = (self.cursor.y as isize - lines.len() as isize) as f32 * char_size.y;
        Vec2::new(x_off, y_off)
    }

    fn init(&mut self, ctx: &egui::Context) {
        self.char_size = Some(get_char_size(ctx));
    }
}

impl eframe::App for TermGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let None = self.char_size {
            self.init(ctx);
            println!("proportions: {:?}\n", self.char_size);
        }
        let mut buf = vec![0u8; 4096];
        match nix::unistd::read(self.fd.as_raw_fd(), &mut buf) {
            Ok(n_bytes) => {
                let bytes = &buf[..n_bytes];
                // TODO: refactor to push chunks of bytes to the buffer
                // by tracking the last special character, continuing while
                // the next character is not a special character, and then
                // pushing bytes[last_special+1..current] to the buffer.
                for byte in bytes.iter().copied() {
                    match byte {
                        b'\n' => {
                            self.buffer.push(b'\n');
                            self.cursor.x = 0;
                            self.cursor.y += 1;
                        }
                        b'\r' => {
                            self.cursor.x = 0;
                        }
                        b'\t' => {
                            self.cursor.x += 4;
                        }
                        _ => {
                            self.buffer.push(byte);
                            self.cursor.x += 1;
                        }
                    }
                }
            }
            Err(Errno::EAGAIN) => {}
            Err(e) => {
                println!("Error reading from fd: {:?}", e);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.input(|state| {
                for event in state.events.iter() {
                    let mut bytes = match event {
                        egui::Event::Key {
                            key: egui::Key::Enter,
                            pressed: true,
                            ..
                        } => b"\n",
                        egui::Event::Text(text) => text.as_bytes(),
                        _ => &[],
                    };
                    while bytes.len() > 0 {
                        match nix::unistd::write(self.fd.as_raw_fd(), &bytes) {
                            Ok(written) => {
                                bytes = &bytes[written..];
                            }
                            Err(Errno::EAGAIN) => continue,
                            Err(_) => break,
                        };
                    }
                }
            });

            let res = ui.label(unsafe { std::str::from_utf8_unchecked(&self.buffer) });

            let bottom = res.rect.bottom();
            let left = res.rect.left();
            let painter = ui.painter();
            let char_size = self.char_size.as_ref().expect("char size to have been set");
            let cursor_offset = self.char_to_cursor_offset(&char_size);

            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::Pos2::new(left + cursor_offset.x, bottom + cursor_offset.y),
                    char_size.clone().into(),
                ),
                0.0,
                egui::Color32::GRAY,
            );
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
            let app = TermGui::new(cc, fd);
            Box::new(app)
        }),
    )
    .map_err(|e| anyhow::anyhow!("{}", e))
}
