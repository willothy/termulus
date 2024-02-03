//! This is a debug GUI for the terminal emulator backend I am building for
//! Sesh. The terminal emulator will be included in this crate, but the GUI
//! stuff is all temporary and for debugging only. The library will be used
//! in sesh to multiplex terminal sessions and allow multiple applications to
//! run in the same terminal window. Currently sesh works by piping the output directly
//! from the server to the client which is very limiting, but this will allow for scrollback,
//! multiple panes, and proper keymappings.
use std::{
    borrow::Cow,
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

pub trait IsTerminator {
    fn is_terminator(&self) -> bool;
}

impl IsTerminator for u8 {
    fn is_terminator(&self) -> bool {
        // FIXME: needs to be implemented
        return false;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalOutput<'a> {
    Ansi(Cow<'a, [u8]>),
    Text(Cow<'a, [u8]>),
}

pub enum CsiParse {
    Row,
    Column,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsiBuilder {
    Text,
    Esc,
    Csi,
}

pub struct OutputParser<'a> {
    state: AnsiBuilder,
    /// A buffer for partially built escape sequenves.
    /// When [`OutputParser::parse`] is called, it will
    /// append incomplete escape sequences to this buffer
    /// and only return complete ones, and then attempt to
    /// resume parsing on the next input.
    partial: Cow<'a, [u8]>,
}

pub const ESC: u8 = 0x1B;
pub const CSI: u8 = 0x5B; // '['

impl<'a> OutputParser<'a> {
    pub fn new() -> Self {
        Self {
            state: AnsiBuilder::Text,
            partial: Cow::Borrowed(&[]),
        }
    }

    pub fn parse(&mut self, bytes: &[u8]) -> Vec<TerminalOutput> {
        let bytes_start = bytes as *const [u8] as *const u8 as usize;
        let mut output: Vec<TerminalOutput> = Vec::new();
        for (i, byte) in bytes.iter().enumerate() {
            match self.state {
                AnsiBuilder::Text => {
                    match byte {
                        &ESC => {
                            if self.partial.len() > 0 {
                                let segment = TerminalOutput::Text(std::mem::replace(
                                    &mut self.partial,
                                    Cow::Borrowed(&[]),
                                ));
                                output.push(segment);
                            }
                            self.state = AnsiBuilder::Esc;
                        }
                        &byte => {
                            // Push to text buffer.
                            // Note thatthere is no actual difference between text and ansi
                            // buffer but the use depends on the state of the parser.
                            match &mut self.partial {
                                Cow::Borrowed(slice) => {
                                    // This is mildly sketchy but I think the logic is sound. These
                                    // should always be slices into the original input so we can
                                    // use pointer arithmetic to get the offset of the slice start
                                    // and the offset of the byte in the slice.
                                    //
                                    // This way we can avoid copying the slice unless it's a
                                    // partial escape sequence that needs to be preserved for the
                                    // next parsing "cycle."
                                    if slice.len() > 0 {
                                        let slice_start =
                                            (*slice) as *const [u8] as *const u8 as usize;
                                        let offset = slice_start - bytes_start;
                                        *slice = unsafe {
                                            (&bytes[offset..slice.len()+1] as *const [u8]).as_ref().expect(
                                            "slice should be valid because it is a slice of the input",
                                            )
                                        };
                                    } else {
                                        *slice = unsafe {
                                            (&bytes[i..i+1] as *const [u8]).as_ref().expect(
                                            "slice should be valid because it is a slice of the input",
                                            )
                                        };
                                    }
                                }
                                Cow::Owned(vec) => {
                                    vec.push(byte);
                                }
                            }
                        }
                    }
                }
                AnsiBuilder::Esc => match byte {
                    &CSI => {
                        self.state = AnsiBuilder::Csi;
                    }
                    byte if byte.is_terminator() => {
                        let segment = TerminalOutput::Ansi(std::mem::replace(
                            &mut self.partial,
                            Cow::Borrowed(&[]),
                        ));
                        output.push(segment);
                        self.state = AnsiBuilder::Text;
                    }
                    &byte => {
                        // push to escape sequence buffer
                        match &mut self.partial {
                            Cow::Borrowed(slice) => {
                                *slice = &slice[..slice.len() + 1];
                            }
                            Cow::Owned(vec) => {
                                vec.push(byte);
                            }
                        }
                    }
                },
                AnsiBuilder::Csi => {
                    // self.partial.push(*byte);
                    panic!(
                        "CSI parsing not implemented yet! Unhandled byte: {} ({:0X}, {})",
                        byte, byte, *byte as char
                    );
                }
            }
        }
        if self.partial.len() > 0 {
            match self.state {
                AnsiBuilder::Text => {
                    let segment = TerminalOutput::Text(std::mem::replace(
                        &mut self.partial,
                        Cow::Borrowed(&[]),
                    ));
                    output.push(segment);
                }
                AnsiBuilder::Esc | AnsiBuilder::Csi => match &self.partial {
                    Cow::Owned(_vec) => {}
                    Cow::Borrowed(slice) => {
                        // let segment = TerminalOutput::Ansi(slice.to_vec().into());
                        // output.push(segment);
                        self.partial = Cow::Owned(slice.to_vec());
                    }
                },
            }
        }
        output
    }
}

#[test]
/// NOTE: this is temporary!! do not keep this test!!
/// this is dependent on an *incorrect* parser and is just for ensuring that
/// the parser is working correctly during development.
fn test_parser() {
    let mut parser = OutputParser::new();
    let input = b"hello\x1B[31mworld\x1B[0m".to_vec();
    let output = parser.parse(&input);
    assert_eq!(output.len(), 1);
    assert_eq!(output[0], TerminalOutput::Text(Cow::Borrowed(b"hello")));
}

pub struct TermGui<'a> {
    parser: OutputParser<'a>,
    buffer: Vec<u8>,
    cursor: CursorPos,
    char_size: Option<Vec2>,
    fd: OwnedFd,
}

impl<'a> TermGui<'a> {
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

    fn update_cursor(&mut self, incoming_bytes: &[u8]) {
        for byte in incoming_bytes.iter() {
            match byte {
                b'\n' => {
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
                    self.cursor.x += 1;
                }
            }
        }
    }

    fn read(&mut self, ctx: &egui::Context) {
        let mut buf = vec![0u8; 4096];
        match nix::unistd::read(self.fd.as_raw_fd(), &mut buf) {
            Ok(n_bytes) => {
                let bytes = &buf[..n_bytes];
                let segments = self.parser.parse(bytes);
                println!("segments: {}", segments.len());
                for segment in segments.into_iter() {
                    match segment {
                        TerminalOutput::Ansi(_seq) => {
                            // panic!("not implemented");
                        }
                        TerminalOutput::Text(text) => {
                            // self.update_cursor(&text);
                            for byte in text.as_ref() {
                                match byte {
                                    b'\n' => {
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
                                        self.cursor.x += 1;
                                    }
                                }
                            }
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
