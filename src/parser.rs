use std::borrow::Cow;

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

pub enum CsiState<'a> {
    Arg1(Cow<'a, [u8]>),
    Arg2(Cow<'a, [u8]>),
    Finished,
    Error,
}

pub struct CsiParser<'a> {
    state: CsiState<'a>,
    arg1: usize,
    arg2: usize,
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

impl<'a> CsiParser<'a> {
    pub fn new() -> Self {
        Self {
            state: CsiState::Arg1(Cow::Borrowed(&[])),
            arg1: 1,
            arg2: 1,
        }
    }

    pub fn push(&mut self, byte: &u8) {
        match &mut self.state {
            CsiState::Arg1(slice) => match byte {
                b'H' => {
                    self.state = CsiState::Finished;
                    return;
                }
                b';' => {
                    let arg1_str = unsafe {
                        // Safety: we know that the slice contains only ascii digits
                        std::str::from_utf8_unchecked(slice)
                    };
                    if arg1_str.len() > 0 {
                        self.arg1 = usize::from_str_radix(arg1_str, 10)
                            .expect("to have already validated the input");
                    } else {
                        self.arg1 = 1;
                    }
                    self.state = CsiState::Arg2(Cow::Borrowed(&[]));
                    return;
                }
                byte if byte.is_ascii_digit() => unsafe {
                    push_byte(slice, byte);
                },
                &byte => {
                    panic!("invalid byte in CSI sequence: {}", byte);
                    self.state = CsiState::Error;
                }
            },
            CsiState::Arg2(slice) => match byte {
                b'H' => {
                    let arg2_str = unsafe {
                        // Safety: we know that the slice contains only ascii digits
                        std::str::from_utf8_unchecked(slice)
                    };
                    if arg2_str.len() > 0 {
                        self.arg2 = usize::from_str_radix(arg2_str, 10)
                            .expect("to have already validated the input");
                    } else {
                        self.arg2 = 1;
                    }
                    // self.state = CsiState::Finished;
                    return;
                }
                byte if byte.is_ascii_digit() => unsafe {
                    push_byte(slice, byte);
                },
                &byte => {
                    panic!("invalid byte in CSI sequence: {}", byte);
                    self.state = CsiState::Error;
                }
            },
            CsiState::Error => panic!(),
            CsiState::Finished => unreachable!(),
        }
        // Take ownership of any incomplete data.
        match &mut self.state {
            CsiState::Arg1(arg1 @ Cow::Borrowed(_)) => {
                if arg1.len() > 0 {
                    *arg1 = Cow::Owned(arg1.to_vec());
                }
            }
            CsiState::Arg2(arg2 @ Cow::Borrowed(_)) => {
                if arg2.len() > 0 {
                    *arg2 = Cow::Owned(arg2.to_vec());
                }
            }
            _ => {}
        }
    }
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

pub const ESC: u8 = 0x1B; // ESCAPE
pub const CSI: u8 = 0x5B; // '['

impl<'a> OutputParser<'a> {
    pub fn new() -> Self {
        Self {
            state: AnsiBuilder::Text,
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
            AnsiBuilder::Text => {
                // Since we are at the end of the input and the input state is text, we can
                // send the text buffer as a segment.
                Some(std::mem::replace(&mut self.partial, Cow::Borrowed(&[])))
            }
            AnsiBuilder::Esc | AnsiBuilder::Csi => match &self.partial {
                // If the partial buffer is borrowed and we have incomplete escape
                // sequences, we need to preserve the buffer for the next parsing
                // cycle by cloning it into an owned buffer that we can mutate.
                //
                // This is a necessity due to the fact that the next input will
                // likely not be located contiguously in memory with the current
                // input and we need to preserve the partial buffer across multiple
                // reads.
                Cow::Borrowed(slice) => {
                    self.partial = Cow::Owned(slice.to_vec());
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
                AnsiBuilder::Text => match byte {
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
                        self.state = AnsiBuilder::Csi;
                    }
                    byte if byte.is_terminator() => {
                        let segment = TerminalOutput::Ansi(std::mem::replace(
                            &mut self.partial,
                            Cow::Borrowed(unsafe {
                                std::slice::from_raw_parts(bytes as *const [u8] as *const u8, 0)
                            }),
                        ));
                        output.push(segment);
                        self.state = AnsiBuilder::Text;
                    }
                    _ => {
                        self.partial_push(byte);
                    }
                },
                AnsiBuilder::Csi => {
                    self.partial_push(byte);
                    // panic!(
                    //     "CSI parsing not implemented yet! Unhandled byte: {} ({:0X}, {})",
                    //     byte, byte, *byte as char
                    // );
                }
            }
        }
        if self.partial.len() > 0 {
            if let Some(text) = self.partial_take() {
                output.push(TerminalOutput::Text(text));
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
    let TerminalOutput::Text(Cow::Borrowed(slice)) = output[0] else {
        panic!("previous assertion should have caught this");
    };
    assert_eq!(slice.len(), 5);
    assert_eq!(parser.partial, Cow::Borrowed(b"31mworld\x1B[0m"));
    assert_eq!(parser.state, AnsiBuilder::Csi);
}
