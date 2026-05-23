//! A line reader that respects both \r and \n as line terminators.
//! This allows progress bars (which use \r) to be emitted immediately.

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

/// A line reader that yields lines on both \n and \r terminators.
/// This allows progress bars (which use \r) to be emitted immediately.
pub struct LineReader {
    buffer: Vec<u8>,
}

impl LineReader {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Read from the input stream and yield lines when \r or \n is encountered.
    /// Returns (line_option, has_more_data)
    /// - line_option: Some(line) if a complete line was found, None if no complete line yet
    /// - has_more_data: true if we should continue reading, false if EOF
    pub async fn read_line<R: AsyncRead + Unpin>(
        &mut self,
        reader: &mut R,
    ) -> io::Result<(Option<String>, bool)> {
        loop {
            // Check if buffer contains carriage return or newline
            if let Some(pos) = self.buffer.iter().position(|&b| b == b'\r' || b == b'\n') {
                let line_bytes = self.buffer.drain(..=pos).collect::<Vec<_>>();
                // Remove the delimiter from the end
                let line_str =
                    String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]).into_owned();
                return Ok((Some(line_str), true));
            }

            // Read more data
            let mut chunk = [0u8; 4096];
            let n = reader.read(&mut chunk).await?;

            if n == 0 {
                // EOF
                if self.buffer.is_empty() {
                    return Ok((None, false));
                } else {
                    // Return remaining buffer as final line
                    let line = String::from_utf8_lossy(&self.buffer).into_owned();
                    self.buffer.clear();
                    return Ok((Some(line), false));
                }
            }

            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }
}

impl Default for LineReader {
    fn default() -> Self {
        Self::new()
    }
}
