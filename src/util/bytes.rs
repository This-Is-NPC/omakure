/// Reads a big-endian binary frame front to back.
///
/// Every short read, overflow or malformed field fails with the one error the
/// caller chose, so a decoder never distinguishes how a frame was malformed.
pub struct ByteReader<'a, E> {
    bytes: &'a [u8],
    offset: usize,
    invalid: E,
}

impl<'a, E: Clone> ByteReader<'a, E> {
    pub fn new(bytes: &'a [u8], invalid: E) -> Self {
        Self {
            bytes,
            offset: 0,
            invalid,
        }
    }

    pub fn take(&mut self, length: usize) -> Result<&'a [u8], E> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| self.invalid.clone())?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| self.invalid.clone())?;
        self.offset = end;
        Ok(value)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], E> {
        self.take(N)?.try_into().map_err(|_| self.invalid.clone())
    }

    pub fn byte(&mut self) -> Result<u8, E> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16, E> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64, E> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    /// `length` bytes of UTF-8.
    pub fn text(&mut self, length: usize) -> Result<String, E> {
        std::str::from_utf8(self.take(length)?)
            .map(str::to_string)
            .map_err(|_| self.invalid.clone())
    }

    /// `length` bytes of UTF-8 containing no NUL or other control character.
    pub fn printable_text(&mut self, length: usize) -> Result<String, E> {
        let value = self.text(length)?;
        if value
            .chars()
            .any(|character| character == '\0' || character.is_control())
        {
            return Err(self.invalid.clone());
        }
        Ok(value)
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_big_endian_fields_in_order() {
        let bytes = [7, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, b'h', b'i'];
        let mut reader = ByteReader::new(&bytes, ());

        assert_eq!(reader.byte(), Ok(7));
        assert_eq!(reader.u16(), Ok(2));
        assert_eq!(reader.u64(), Ok(3));
        assert_eq!(reader.remaining(), 2);
        assert_eq!(reader.text(2).as_deref(), Ok("hi"));
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn short_reads_and_overflow_fail_with_the_chosen_error() {
        let mut reader = ByteReader::new(&[1], "invalid");

        assert_eq!(reader.u16(), Err("invalid"));
        assert_eq!(reader.take(usize::MAX), Err("invalid"));
    }

    #[test]
    fn printable_text_refuses_control_characters() {
        assert_eq!(ByteReader::new(b"a\tb", ()).text(3).as_deref(), Ok("a\tb"));
        assert_eq!(ByteReader::new(b"a\tb", ()).printable_text(3), Err(()));
        assert_eq!(ByteReader::new(b"a\0b", ()).printable_text(3), Err(()));
        assert_eq!(ByteReader::new(&[0xff], ()).text(1), Err(()));
    }
}
