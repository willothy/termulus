use std::borrow::Cow;

pub trait IsTerminator {
    fn is_csi_terminator(&self) -> bool;
}

impl IsTerminator for u8 {
    fn is_csi_terminator(&self) -> bool {
        match self {
            b'A'..=b'H' => true, // Cursor position
            b'J' | b'K' => true, // Erase display/line
            b'S' | b'T' => true, // Scroll up/down
            b'f' => true,        // Horizontal vertical position (?)
            b'm' => true,        // Select Graphic Rendition (SGR)
            b's' | b'u' => true, // Save/restore cursor position
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalOutput<'a> {
    Ansi(Cow<'a, [u8]>),
    Text(Cow<'a, [u8]>),
    SetCursorPos { x: usize, y: usize },
    ClearForwards,
    ClearBackwards,
    ClearAll,
    RestoreCursorPos,
    SaveCursorPos,
    // I don't have scrollback yet
    // ClearAllAndScrollback
}

/// Push a byte into a Cow<'a, [u8]>
///
/// The caller must ensure that if the Cow is borrowed, the slice is not
/// longer than the memory it references.
///
/// As long as the arguments satisfy the following conditions, this function is safe
/// to call:
///
/// - `&byte >= &slice` (the byte is within the slice or after it)
/// - `&byte >= &input[0] && &byte < &input[input.len()]` (the byte is within the input)
/// - `&slice >= &input[0] && &slice <= &input[input.len()]` (the slice is within the input)
/// - `&slice[slice.len()] <= &input[input.len()]` (the slice  is within the input)
unsafe fn push_byte(slice: &mut Cow<'_, [u8]>, byte: &u8) {
    match slice {
        Cow::Borrowed(slice) => {
            assert!(byte as *const u8 >= *slice as *const [u8] as *const u8);
            // // These assertions cannot be made at the moment because we do not have the original
            // // input in the Csi parser, and the original input is not necessarily a slice or contiguous
            // // in memory.
            // assert!(byte >= &input[0] && byte < &input[input.len()]);
            // assert!(&slice[0] >= &input[0] && &slice[0] <= &input[input.len()]);
            // assert!(&slice[slice.len()] <= &input[input.len()]);
            let len = slice.len();
            if len > 0 {
                // If the slice is borrowed and non-empty, the byte should *always*
                // be located directly after the end of the slice.
                assert_eq!(
                    byte as *const u8 as usize,
                    *slice as *const [u8] as *const u8 as usize + len
                );
                let start = *slice as *const [u8] as *const u8;
                *slice = unsafe { std::slice::from_raw_parts(start, len + 1) };
            } else {
                *slice = unsafe { std::slice::from_raw_parts(byte, 1) };
            }
        }
        Cow::Owned(vec) => {
            vec.push(*byte);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsiState<'a> {
    Argument(Cow<'a, [u8]>),
    Finished(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsiParser<'a> {
    state: CsiState<'a>,
    args: Vec<usize>,
}

impl<'a> CsiParser<'a> {
    pub fn new() -> Self {
        Self {
            state: CsiState::Argument(Cow::Borrowed(&[])),
            args: Vec::new(),
        }
    }

    pub fn has_incomplete_output(&self) -> bool {
        match &self.state {
            CsiState::Argument(slice) => slice.len() > 0,
            CsiState::Finished(_) => false,
        }
    }

    pub fn take_incomplete(&mut self) {
        // Take ownership of any incomplete data.
        match &mut self.state {
            CsiState::Argument(arg @ Cow::Borrowed(_)) => {
                if arg.len() > 0 {
                    *arg = Cow::Owned(arg.to_vec());
                }
            }
            _ => {}
        }
    }

    pub fn push(&mut self, byte: &u8) {
        if let CsiState::Finished(_) = self.state {
            panic!("attempted to push byte into finished CSI sequence");
        }

        fn accumulate(slice: &Cow<'_, [u8]>) -> Option<usize> {
            if slice.len() > 0 {
                let str = unsafe {
                    // Safety: we know that the slice contains only ascii digits
                    std::str::from_utf8_unchecked(slice)
                };
                Some(usize::from_str_radix(str, 10).expect("to have already validated the input"))
            } else {
                None
            }
        }

        match &mut self.state {
            CsiState::Argument(slice) => match byte {
                byte if byte.is_csi_terminator() => {
                    if let Some(arg) = accumulate(slice) {
                        self.args.push(arg);
                    }
                    self.state = CsiState::Finished(*byte);
                }
                b';' => {
                    if let Some(arg) = accumulate(slice) {
                        self.args.push(arg);
                    }
                    self.state = CsiState::Argument(Cow::Borrowed(&[]));
                }
                byte if byte.is_ascii_digit() => unsafe {
                    push_byte(slice, byte);
                },
                byte => {
                    //NOTE: temporary
                    // We need to take ownership of the slice when we encounted invalid data
                    // because the valid data is no longer contiguous in memory as it is separated
                    // by invalid data.
                    match slice {
                        Cow::Borrowed(ref s) => {
                            *slice = Cow::Owned(s.to_vec());
                        }
                        Cow::Owned(_) => {}
                    };
                    println!(
                        "invalid byte in CSI sequence: {} ('{}')",
                        byte, *byte as char
                    );
                }
            },
            CsiState::Finished(_) => unreachable!(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnsiBuilder<'a> {
    Empty,
    Esc,
    Csi(CsiParser<'a>),
}

pub struct OutputParser<'a> {
    state: AnsiBuilder<'a>,
    /// A buffer for partially built escape sequences.
    /// When [`OutputParser::parse`] is called, it will
    /// append incomplete escape sequences to this buffer
    /// and only return complete ones, and then attempt to
    /// resume parsing on the next input.
    partial: Cow<'a, [u8]>,
}

pub const ESC: u8 = 0x1B; // ESCAPE
pub const CSI: u8 = 0x5B; // '['

impl<'a> OutputParser<'a> {
    pub fn new() -> Self {
        Self {
            state: AnsiBuilder::Empty,
            partial: Cow::Borrowed(&[]),
        }
    }

    fn partial_push(&mut self, byte: &u8) {
        // Push to partial buffer.
        // Note that there is no actual difference between text and ansi
        // buffer but the use depends on the state of the parser.
        //
        // This is mildly sketchy but I think the logic is sound. These
        // should always be slices into the original input so we can
        // use pointer arithmetic to get the offset of the slice start
        // and the offset of the byte in the slice.
        //
        // This way we can avoid copying the slice unless it's a
        // partial escape sequence that needs to be preserved for the
        // next parsing "cycle."
        unsafe {
            push_byte(&mut self.partial, byte);
        }
    }

    fn partial_take(&mut self) -> Option<Cow<'a, [u8]>> {
        match self.state {
            AnsiBuilder::Empty => {
                // Since we are at the end of the input and the input state is text, we can
                // send the text buffer as a segment.
                if self.partial.len() > 0 {
                    Some(std::mem::replace(&mut self.partial, Cow::Borrowed(&[])))
                } else {
                    None
                }
            }
            AnsiBuilder::Csi(ref mut csi) => {
                if csi.has_incomplete_output() {
                    csi.take_incomplete();
                }
                None
            }
            AnsiBuilder::Esc => match &self.partial {
                // If the partial buffer is borrowed and we have incomplete escape
                // sequences, we need to preserve the buffer for the next parsing
                // cycle by cloning it into an owned buffer that we can mutate.
                //
                // This is a necessity due to the fact that the next input will
                // likely not be located contiguously in memory with the current
                // input and we need to preserve the partial buffer across multiple
                // reads.
                Cow::Borrowed(slice) => {
                    if slice.len() > 0 {
                        let vec = slice.to_vec();
                        self.partial = Cow::Owned(vec);
                    }
                    None
                }
                // If the partial buffer is owned, we don't need to do anything.
                Cow::Owned(_vec) => None,
            },
        }
    }

    pub fn parse(&mut self, bytes: &[u8]) -> Vec<TerminalOutput> {
        if self.partial.len() == 0 {
            self.partial = Cow::Borrowed(unsafe {
                std::slice::from_raw_parts(bytes as *const [u8] as *const u8, 0)
            });
        }
        let mut output: Vec<TerminalOutput> = Vec::new();
        for byte in bytes {
            match self.state {
                AnsiBuilder::Empty => match byte {
                    &ESC => {
                        if self.partial.len() > 0 {
                            let segment = TerminalOutput::Text(std::mem::replace(
                                &mut self.partial,
                                Cow::Borrowed(unsafe {
                                    std::slice::from_raw_parts(bytes as *const [u8] as *const u8, 0)
                                }),
                            ));
                            output.push(segment);
                        }
                        self.state = AnsiBuilder::Esc;
                    }
                    _ => {
                        self.partial_push(byte);
                    }
                },
                AnsiBuilder::Esc => match byte {
                    &CSI => {
                        self.state = AnsiBuilder::Csi(CsiParser::new());
                    }
                    byte if byte.is_csi_terminator() => {
                        unreachable!()
                        // let segment = TerminalOutput::Ansi(std::mem::replace(
                        //     &mut self.partial,
                        //     Cow::Borrowed(unsafe {
                        //         std::slice::from_raw_parts(bytes as *const [u8] as *const u8, 0)
                        //     }),
                        // ));
                        // output.push(segment);
                        // self.state = AnsiBuilder::Empty;
                    }
                    _ => {
                        self.partial_push(byte);
                    }
                },
                AnsiBuilder::Csi(ref mut parser) => {
                    parser.push(byte);
                    match parser.state {
                        CsiState::Argument(_) => {}
                        CsiState::Finished(b'H') => {
                            // move cursor to position
                            output.push(TerminalOutput::SetCursorPos {
                                x: parser.args.pop().unwrap_or(1),
                                y: parser.args.pop().unwrap_or(1),
                            });
                            self.state = AnsiBuilder::Empty;
                        }
                        CsiState::Finished(b'J') => {
                            // move cursor to position
                            let command = match parser.args.pop() {
                                Some(0) | None => TerminalOutput::ClearForwards,
                                Some(1) => TerminalOutput::ClearBackwards,
                                Some(2) => TerminalOutput::ClearAll,
                                Some(3..) => panic!("invalid argument for J command"),
                            };
                            output.push(command);
                            self.state = AnsiBuilder::Empty;
                        }
                        CsiState::Finished(b's') => {
                            output.push(TerminalOutput::SaveCursorPos);
                            self.state = AnsiBuilder::Empty;
                        }
                        CsiState::Finished(b'u') => {
                            output.push(TerminalOutput::RestoreCursorPos);
                            self.state = AnsiBuilder::Empty;
                        }
                        CsiState::Finished(terminator) => {
                            // TODO: temporary
                            output.push(TerminalOutput::Ansi(Cow::Borrowed(&[])));
                            println!(
                                "unhandled CSI terminator: {:X} {}",
                                terminator, terminator as char
                            );
                            self.state = AnsiBuilder::Empty;
                        }
                    }
                }
            }
        }
        if let Some(text) = self.partial_take() {
            output.push(TerminalOutput::Text(text));
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
    let input = b"hello\x1B[1;12Hworld\x1b[0".to_vec();
    let output = parser.parse(&input);
    assert_eq!(output.len(), 3);
    assert_eq!(output[0], TerminalOutput::Text(Cow::Borrowed(b"hello")));
    let TerminalOutput::Text(Cow::Borrowed(slice)) = output[0] else {
        panic!("previous assertion should have caught this");
    };
    assert_eq!(slice.len(), 5);
    assert_eq!(output[1], TerminalOutput::SetCursorPos { x: 12, y: 1 });
    assert_eq!(output[2], TerminalOutput::Text(Cow::Borrowed(b"world")));
    let TerminalOutput::Text(Cow::Borrowed(slice)) = output[2] else {
        panic!("previous assertion should have caught this");
    };
    assert_eq!(slice.len(), 5);
    assert_eq!(parser.partial.len(), 0);
    match &parser.state {
        AnsiBuilder::Csi(csi_parser) => {
            // the \x1B[ are not inclued in the buffer
            assert_eq!(csi_parser.state, CsiState::Argument(Cow::Borrowed(b"0")));
        }
        _ => panic!("parser state should be AnsiBuilder::Csi"),
    }
    let input2 = b"m";
    let output2 = parser.parse(input2);
    assert_eq!(output2.len(), 1);
    assert_eq!(parser.partial.len(), 0);
    match &parser.state {
        AnsiBuilder::Empty => {}
        _ => panic!("parser state should be AnsiBuilder::Empty"),
    }
}
