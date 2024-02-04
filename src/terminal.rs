use std::os::fd::{AsRawFd, OwnedFd};

use crate::parser::{OutputParser, TerminalOutput};
use anyhow::Result;
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

    pub fn to_buffer_pos(&self, buffer: &[u8]) -> usize {
        buffer
            .split(|b| *b == b'\n')
            .take(self.y)
            .map(|line| line.len())
            .sum::<usize>()
            + self.x
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
    saved_cursor: Option<CursorPos>,
    fd: OwnedFd,
}

impl<'a> Terminal<'a> {
    // TODO: write a builder that spawns a new process so the fd doesn't need to be exposed
    // to the rest of the program.
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
            saved_cursor: None,
            buffer: Vec::new(),
        }
    }

    pub fn get_window_size(&self) -> Result<nix::pty::Winsize> {
        // This defines the raw ioctl function that we can use to get the window size
        nix::ioctl_read_bad!(raw_get_win_size, nix::libc::TIOCGWINSZ, nix::pty::Winsize);

        let mut ws = nix::pty::Winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0, // unused
            ws_ypixel: 0, // unused
        };

        unsafe {
            raw_get_win_size(self.fd.as_raw_fd(), &mut ws)?;
        }

        Ok(ws)
    }

    pub fn set_window_size(&mut self, size: &nix::pty::Winsize) -> Result<()> {
        // This defines the raw ioctl function that we can use to get the window size
        nix::ioctl_write_ptr_bad!(raw_set_win_size, nix::libc::TIOCSWINSZ, nix::pty::Winsize);

        unsafe {
            raw_set_win_size(self.fd.as_raw_fd(), size)?;
        }
        Ok(())
    }

    /// Access the buffer as a &str. This function is safe because
    /// we know that all non-printable characters have been removed by
    /// the parser.
    pub fn buffer(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(&self.buffer) }
    }

    pub fn cursor_pos(&self) -> &CursorPos {
        &self.cursor
    }

    pub fn char_to_cursor_offset(&self) -> Vec2 {
        println!("Retrieved cursor pos: {}, {}", self.cursor.x, self.cursor.y);
        let lines = self.buffer.split(|b| *b == b'\n').count();

        let x_off = self.cursor.x as f32;
        let y_off = (self.cursor.y as isize - lines as isize) as f32;
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
                            println!("updated cursor to {}, {}", self.cursor.x, self.cursor.y);
                            self.buffer.extend_from_slice(&text);
                        }
                        TerminalOutput::SetCursorPos { x, y } => {
                            self.cursor.x = x - 1;
                            self.cursor.y = y - 1;
                            println!("need to set cursor to x: {}, y: {}", x, y);
                        }
                        TerminalOutput::ClearForwards => {
                            let pos = self.cursor.to_buffer_pos(&self.buffer);
                            self.buffer.drain(pos..);
                        }
                        TerminalOutput::ClearBackwards => {
                            let pos = self.cursor.to_buffer_pos(&self.buffer);
                            self.buffer.drain(..pos);
                        }
                        TerminalOutput::ClearAll => {
                            self.buffer.clear();
                            self.cursor.x = 0;
                            self.cursor.y = 0;
                        }
                        TerminalOutput::RestoreCursorPos => {
                            if let Some(saved) = self.saved_cursor.take() {
                                self.cursor = saved;
                            }
                        }
                        TerminalOutput::SaveCursorPos => {
                            self.saved_cursor = Some(self.cursor.clone());
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
