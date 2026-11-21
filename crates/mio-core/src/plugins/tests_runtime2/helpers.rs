use std::io::{Read, Write};

use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

pub(super) struct BlockingReader(pub(super) DuplexStream, pub(super) tokio::runtime::Handle);

impl Read for BlockingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.1.block_on(self.0.read(buf))
    }
}

pub(super) struct BlockingWriter(pub(super) DuplexStream, pub(super) tokio::runtime::Handle);

impl Write for BlockingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.1.block_on(self.0.write(buf))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.1.block_on(self.0.flush())
    }
}
