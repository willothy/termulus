use std::os::fd::{AsRawFd, OwnedFd};

use crate::parser::{OutputParser, TerminalOutput};
use egui::{self, TextStyle, Vec2};
use nix::{
    errno::Errno,
    fcntl::{FcntlArg, OFlag},
    libc::O_ACCMODE,
};

#[derive(Debug, Clone)]
pub struct CursorPos {
    x: usize,
    y: usize,
}

impl CursorPos {
    fn new(x: usize, y: usize) -> Self {
        Self { x, y }
    }

    pub fn update(&mut self, incoming: &[u8]) {
        for byte in incoming.iter() {
            match byte {
                b'\n' => {
                    self.x = 0;
                    self.y += 1;
                }
                b'\r' => {
                    self.x = 0;
                }
                b'\t' => {
                    self.x += 4;
                }
                _ => {
                    self.x += 1;
                }
            }
        }
    }
}

pub trait GetCharSize {
    fn get_char_size(&self, style: &TextStyle) -> Vec2;
}

impl GetCharSize for egui::Context {
    fn get_char_size(&self, style: &TextStyle) -> Vec2 {
        let font_id = self.style().text_styles[style].clone();
        self.fonts(|fonts| {
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
}

pub struct TermGui<'a> {
    parser: OutputParser<'a>,
    buffer: Vec<u8>,
    cursor: CursorPos,
    char_size: Option<Vec2>,
    fd: OwnedFd,
}

impl<'a> TermGui<'a> {
    pub fn new(cc: &eframe::CreationContext<'_>, fd: OwnedFd) -> Self {
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
            parser: OutputParser::new(),
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
        self.char_size = Some(ctx.get_char_size(&TextStyle::Monospace));
    }

    fn read(&mut self, ctx: &egui::Context) {
        let mut buf = vec![0u8; 4096];
        match nix::unistd::read(self.fd.as_raw_fd(), &mut buf) {
            Ok(n_bytes) => {
                let bytes = &buf[..n_bytes];
                let segments = self.parser.parse(bytes);
                for segment in segments {
                    match segment {
                        TerminalOutput::Ansi(_seq) => {
                            // panic!("not implemented");
                        }
                        TerminalOutput::Text(text) => {
                            self.cursor.update(&text);
                            self.buffer.extend_from_slice(&text);
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
    }
}

impl<'a> eframe::App for TermGui<'a> {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let None = self.char_size {
            self.init(ctx);
            println!("proportions: {:?}\n", self.char_size);
        }
        self.read(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.input(|state| {
                for event in state.events.iter() {
                    let mut bytes = match event {
                        egui::Event::Key {
                            key: egui::Key::Enter,
                            pressed: true,
                            ..
                        } => b"\n".as_slice(),
                        egui::Event::Text(text) => text.as_bytes(),
                        _ => b"".as_slice(),
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
