pub struct Form<'a> {
    remaining: &'a str,
}

impl<'a> Form<'a> {
    pub fn new(input: &'a str) -> Self {
        Self { remaining: input }
    }
}

impl<'a> Iterator for Form<'a> {
    type Item = (Decoded<'a>, Decoded<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }

        let (pair, rest) = match self.remaining.split_once('&') {
            Some((pair, rest)) => (pair, rest),
            None => (self.remaining, ""),
        };

        self.remaining = rest;

        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));

        Some((Decoded::new(key), Decoded::new(value)))
    }
}

pub struct Decoded<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Decoded<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
        }
    }
}

impl Iterator for Decoded<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        if self.pos >= self.bytes.len() {
            return None;
        }

        let b = self.bytes[self.pos];
        self.pos += 1;

        match b {
            b'+' => Some(b' '),
            b'%' if self.pos + 1 < self.bytes.len() => {
                let hi = hex(self.bytes[self.pos]);
                let lo = hex(self.bytes[self.pos + 1]);

                if let (Some(hi), Some(lo)) = (hi, lo) {
                    self.pos += 2;
                    Some((hi << 4) | lo)
                } else {
                    Some(b'%')
                }
            }
            _ => Some(b),
        }
    }
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}