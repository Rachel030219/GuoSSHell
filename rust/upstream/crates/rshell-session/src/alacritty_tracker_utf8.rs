#[derive(Clone, Copy, Default)]
pub(super) struct Utf8Decoder {
    bytes: [u8; 4],
    length: usize,
    expected: usize,
}

pub(super) enum Decoded {
    Pending,
    Char(char),
    Invalid,
}

impl Utf8Decoder {
    pub(super) fn is_empty(self) -> bool {
        self.expected == 0
    }

    pub(super) fn push(&mut self, byte: u8) -> Decoded {
        if self.expected == 0 {
            let expected = match byte {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => return Decoded::Invalid,
            };
            self.bytes[0] = byte;
            self.length = 1;
            self.expected = expected;
            return Decoded::Pending;
        }
        if !(0x80..=0xbf).contains(&byte) {
            self.reset();
            return Decoded::Invalid;
        }
        self.bytes[self.length] = byte;
        self.length += 1;
        if self.length != self.expected {
            return Decoded::Pending;
        }
        let decoded = std::str::from_utf8(&self.bytes[..self.expected])
            .ok()
            .and_then(|text| text.chars().next())
            .map_or(Decoded::Invalid, Decoded::Char);
        self.reset();
        decoded
    }

    fn reset(&mut self) {
        self.length = 0;
        self.expected = 0;
    }
}
