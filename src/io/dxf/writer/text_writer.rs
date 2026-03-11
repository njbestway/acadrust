//! ASCII DXF writer

use std::io::Write;
use crate::error::Result;
use crate::types::Handle;
use super::stream_writer::DxfStreamWriter;

/// ASCII DXF stream writer.
///
/// Uses CR/LF (`\r\n`) line endings as required by the DXF text format
/// specification.
pub struct DxfTextWriter<W: Write> {
    writer: W,
    /// Reusable stack buffer for formatting numbers without heap allocation.
    fmt_buf: [u8; 48],
}

impl<W: Write> DxfTextWriter<W> {
    /// Create a new ASCII DXF writer
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            fmt_buf: [0u8; 48],
        }
    }

    /// Write a DXF group code (right-aligned in 3-character field) followed by CRLF.
    /// Uses itoa for zero-allocation integer formatting.
    #[inline]
    fn write_code(&mut self, code: i32) -> Result<()> {
        let mut ibuf = itoa::Buffer::new();
        let s = ibuf.format(code);
        // Right-align in 3-char field: pad with spaces
        let pad = if s.len() < 3 { 3 - s.len() } else { 0 };
        // Build the line in fmt_buf: spaces + digits + \r\n
        let len = pad + s.len() + 2;
        self.fmt_buf[..pad].fill(b' ');
        self.fmt_buf[pad..pad + s.len()].copy_from_slice(s.as_bytes());
        self.fmt_buf[pad + s.len()] = b'\r';
        self.fmt_buf[pad + s.len() + 1] = b'\n';
        self.writer.write_all(&self.fmt_buf[..len])?;
        Ok(())
    }

    /// Write a value string followed by CRLF.
    #[inline]
    fn write_value_crlf(&mut self, value: &[u8]) -> Result<()> {
        self.writer.write_all(value)?;
        self.writer.write_all(b"\r\n")?;
        Ok(())
    }

    /// Format an i16/i32 right-aligned in a 6-character field into fmt_buf.
    /// Returns the slice length.
    #[inline]
    fn format_right6(&mut self, value: i32) -> usize {
        let mut ibuf = itoa::Buffer::new();
        let s = ibuf.format(value);
        let slen = s.len();
        let pad = if slen < 6 { 6 - slen } else { 0 };
        let total = pad + slen + 2; // +2 for \r\n
        self.fmt_buf[..pad].fill(b' ');
        self.fmt_buf[pad..pad + slen].copy_from_slice(s.as_bytes());
        self.fmt_buf[pad + slen] = b'\r';
        self.fmt_buf[pad + slen + 1] = b'\n';
        total
    }

    /// Format f64 with 16 decimal places, trimming trailing zeros (keeping at
    /// least one digit after the decimal point). Written directly into fmt_buf
    /// to avoid heap allocation.
    #[inline]
    fn format_double(&mut self, value: f64) -> usize {
        use std::io::Cursor;
        let mut cursor = Cursor::new(&mut self.fmt_buf[..]);
        // write! into a Cursor<&mut [u8]> does not allocate
        let _ = write!(cursor, "{:.16}", value);
        let mut len = cursor.position() as usize;
        // Trim trailing '0's (but keep at least one digit after '.')
        while len > 1 && self.fmt_buf[len - 1] == b'0' {
            len -= 1;
        }
        // If we trimmed down to just '.', add back a '0'
        if len > 0 && self.fmt_buf[len - 1] == b'.' {
            self.fmt_buf[len] = b'0';
            len += 1;
        }
        // Append CRLF
        self.fmt_buf[len] = b'\r';
        self.fmt_buf[len + 1] = b'\n';
        len + 2
    }

    /// Get the inner writer
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: Write> DxfStreamWriter for DxfTextWriter<W> {
    #[inline]
    fn write_string(&mut self, code: i32, value: &str) -> Result<()> {
        self.write_code(code)?;
        self.write_value_crlf(value.as_bytes())?;
        Ok(())
    }

    #[inline]
    fn write_byte(&mut self, code: i32, value: u8) -> Result<()> {
        self.write_code(code)?;
        let mut ibuf = itoa::Buffer::new();
        let s = ibuf.format(value);
        self.write_value_crlf(s.as_bytes())?;
        Ok(())
    }

    #[inline]
    fn write_i16(&mut self, code: i32, value: i16) -> Result<()> {
        self.write_code(code)?;
        let len = self.format_right6(value as i32);
        self.writer.write_all(&self.fmt_buf[..len])?;
        Ok(())
    }

    #[inline]
    fn write_i32(&mut self, code: i32, value: i32) -> Result<()> {
        self.write_code(code)?;
        let len = self.format_right6(value);
        self.writer.write_all(&self.fmt_buf[..len])?;
        Ok(())
    }

    #[inline]
    fn write_i64(&mut self, code: i32, value: i64) -> Result<()> {
        self.write_code(code)?;
        let mut ibuf = itoa::Buffer::new();
        let s = ibuf.format(value);
        self.write_value_crlf(s.as_bytes())?;
        Ok(())
    }

    #[inline]
    fn write_double(&mut self, code: i32, value: f64) -> Result<()> {
        self.write_code(code)?;
        let len = self.format_double(value);
        self.writer.write_all(&self.fmt_buf[..len])?;
        Ok(())
    }

    #[inline]
    fn write_bool(&mut self, code: i32, value: bool) -> Result<()> {
        self.write_code(code)?;
        // "     0\r\n" or "     1\r\n" — always 8 bytes
        if value {
            self.writer.write_all(b"     1\r\n")?;
        } else {
            self.writer.write_all(b"     0\r\n")?;
        }
        Ok(())
    }

    fn write_handle(&mut self, code: i32, handle: Handle) -> Result<()> {
        self.write_code(code)?;
        // Format handle value as uppercase hex directly into fmt_buf
        let val = handle.value();
        if val == 0 {
            self.write_value_crlf(b"0")?;
        } else {
            // Upper-case hex: max u64 = 16 hex digits
            let mut pos = 16usize; // start from end of a 16-byte region
            let mut v = val;
            while v > 0 {
                pos -= 1;
                let digit = (v & 0xF) as u8;
                self.fmt_buf[pos] = if digit < 10 { b'0' + digit } else { b'A' + digit - 10 };
                v >>= 4;
            }
            let hex_len = 16 - pos;
            // Shift to start of buffer and append CRLF
            self.fmt_buf.copy_within(pos..16, 0);
            self.fmt_buf[hex_len] = b'\r';
            self.fmt_buf[hex_len + 1] = b'\n';
            self.writer.write_all(&self.fmt_buf[..hex_len + 2])?;
        }
        Ok(())
    }

    fn write_binary(&mut self, code: i32, data: &[u8]) -> Result<()> {
        self.write_code(code)?;
        const HEX_CHARS: &[u8; 16] = b"0123456789ABCDEF";
        // Write in chunks using fmt_buf to reduce write calls
        let chunk_bytes = self.fmt_buf.len() / 2; // 2 hex chars per byte
        for chunk in data.chunks(chunk_bytes) {
            let mut pos = 0;
            for &byte in chunk {
                self.fmt_buf[pos] = HEX_CHARS[(byte >> 4) as usize];
                self.fmt_buf[pos + 1] = HEX_CHARS[(byte & 0x0F) as usize];
                pos += 2;
            }
            self.writer.write_all(&self.fmt_buf[..pos])?;
        }
        self.writer.write_all(b"\r\n")?;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Vector3;
    use super::super::stream_writer::DxfStreamWriterExt;
    
    #[test]
    fn test_write_string() {
        let mut buf = Vec::new();
        {
            let mut writer = DxfTextWriter::new(&mut buf);
            writer.write_string(0, "LINE").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "  0\r\nLINE\r\n");
    }
    
    #[test]
    fn test_write_code_formatting() {
        let mut buf = Vec::new();
        {
            let mut writer = DxfTextWriter::new(&mut buf);
            writer.write_i16(5, 100).unwrap();
            writer.write_i16(62, 7).unwrap();
            writer.write_i16(100, 1).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        // Codes should be right-aligned in 3-character field
        assert!(output.starts_with("  5\r\n"));
        assert!(output.contains(" 62\r\n"));
        assert!(output.contains("100\r\n"));
    }
    
    #[test]
    fn test_write_point3d() {
        let mut buf = Vec::new();
        {
            let mut writer = DxfTextWriter::new(&mut buf);
            writer.write_point3d(10, Vector3::new(1.0, 2.0, 3.0)).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains(" 10\r\n"));
        assert!(output.contains("1.0\r\n"));
        assert!(output.contains(" 20\r\n"));
        assert!(output.contains("2.0\r\n"));
        assert!(output.contains(" 30\r\n"));
        assert!(output.contains("3.0\r\n"));
    }
    
    #[test]
    fn test_write_handle() {
        let mut buf = Vec::new();
        {
            let mut writer = DxfTextWriter::new(&mut buf);
            writer.write_handle(5, Handle::new(255)).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("FF\r\n"));
    }
}

