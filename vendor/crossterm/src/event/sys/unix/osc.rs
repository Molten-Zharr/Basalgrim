//! Optional bounded OSC 72 framing. Other OSCs and oversized packets are discarded through ST.
#[derive(Debug, Default)]
pub(crate) struct Osc {
    bytes: Vec<u8>,
    escaped: bool,
    overflow: bool,
}
impl Osc {
    pub(crate) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
    pub(crate) fn push(&mut self, byte: u8) -> Option<Option<Vec<u8>>> {
        if byte == 7 || (self.escaped && byte == b'\\') {
            if self.escaped {
                self.bytes.pop();
            }
            return Some(
                (!self.overflow && self.bytes.starts_with(b"72;"))
                    .then(|| self.bytes[3..].to_vec()),
            );
        }
        self.escaped = byte == 27;
        if self.bytes.len() < 8192 {
            self.bytes.push(byte);
        } else {
            self.overflow = true;
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunk_boundaries_preserve_packets_and_oversized_data_never_becomes_keys() {
        let mut osc = Osc::default();
        assert!(osc.is_empty());
        for byte in b"72;t=m:i=42:x=3:y=4;text/uri-list\x1b" {
            assert!(osc.push(*byte).is_none());
        }
        assert_eq!(
            osc.push(b'\\'),
            Some(Some(b"t=m:i=42:x=3:y=4;text/uri-list".to_vec()))
        );
        let mut osc = Osc::default();
        for byte in b"72;"
            .iter()
            .copied()
            .chain(std::iter::repeat_n(b'x', 20000))
            .chain([27])
        {
            assert!(osc.push(byte).is_none());
        }
        assert_eq!(osc.bytes.len(), 8192);
        assert_eq!(osc.push(b'\\'), Some(None));
    }
}
