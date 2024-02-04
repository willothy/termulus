use std::os::fd::{AsRawFd, OwnedFd};

use crate::parser::{OutputParser, TerminalOutput};
use egui::{self, Vec2};
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

pub struct Terminal<'a> {
    parser: OutputParser<'a>,
    buffer: Vec<u8>,
    cursor: CursorPos,
    fd: OwnedFd,
}

impl<'a> Terminal<'a> {
    pub fn new(fd: OwnedFd) -> Self {
        let flags = nix::fcntl::fcntl(fd.as_raw_fd(), FcntlArg::F_GETFL).expect("fcntl");
        let mut flags = OFlag::from_bits(flags & O_ACCMODE).unwrap();
        // set fd to nonblocking
        flags.set(OFlag::O_NONBLOCK, true);
        nix::fcntl::fcntl(fd.as_raw_fd(), FcntlArg::F_SETFL(flags)).expect("fcntl");
        Self {
            fd,
            parser: OutputParser::new(),
            cursor: CursorPos::new(0, 0),
            buffer: Vec::new(),
        }
    }

    /// Access the buffer as a &str. This functin is safe because
    /// we know that all non-printable characters have been removed by
    /// the parser.
    pub fn buffer(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(&self.buffer) }
    }

    pub fn char_to_cursor_offset(&self /* , char_size: &Vec2 */) -> Vec2 {
        let lines = self.buffer.split(|b| *b == b'\n').collect::<Vec<_>>();

        let x_off = self.cursor.x as f32; // * char_size.x;
        let y_off = (self.cursor.y as isize - lines.len() as isize) as f32; // * char_size.y;
        Vec2::new(x_off, y_off)
    }

    pub fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut bytes = &bytes[..];
        while bytes.len() > 0 {
            match nix::unistd::write(self.fd.as_raw_fd(), &bytes) {
                Ok(written) => {
                    bytes = &bytes[written..];
                }
                Err(Errno::EAGAIN) => {
                    continue;
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Error writing to fd: {:?}", e));
                }
            };
        }
        Ok(())
    }

    pub fn read(&mut self) -> anyhow::Result<()> {
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
                        TerminalOutput::SetCursorPos { x, y } => {
                            panic!("need to set cursor to x: {}, y: {}", x, y);
                        }
                    }
                }
                Ok(())
            }
            Err(Errno::EAGAIN) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("Error reading from fd: {:?}", e)),
        }
    }
}
