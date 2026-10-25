//! Route tracing through Miniquad's real console imports. `tracing-wasm`
//! requires wasm-bindgen glue, which the Macroquad loader does not install.
use std::io::{self, Write};
use tracing::{Level, Metadata};
use tracing_subscriber::fmt::MakeWriter;

struct ConsoleWriter {
    level: Level,
    bytes: Vec<u8>,
}

impl Write for ConsoleWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for ConsoleWriter {
    fn drop(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        let message = String::from_utf8_lossy(&self.bytes);
        let message = message.trim_end();
        match self.level {
            Level::ERROR => macroquad::logging::error!("{}", message),
            Level::WARN => macroquad::logging::warn!("{}", message),
            Level::INFO => macroquad::logging::info!("{}", message),
            Level::DEBUG => macroquad::logging::debug!("{}", message),
            Level::TRACE => macroquad::logging::trace!("{}", message),
        }
    }
}

struct Console;
impl<'a> MakeWriter<'a> for Console {
    type Writer = ConsoleWriter;

    fn make_writer(&'a self) -> ConsoleWriter {
        ConsoleWriter {
            level: Level::INFO,
            bytes: Vec::new(),
        }
    }

    fn make_writer_for(&'a self, metadata: &Metadata<'_>) -> ConsoleWriter {
        ConsoleWriter {
            level: *metadata.level(),
            bytes: Vec::new(),
        }
    }
}

pub(crate) fn init() {
    // Browser console timestamps accompany each event. SystemTime is not
    // available on this WASM target, so do not use fmt's native clock.
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_writer(Console)
        .without_time()
        .with_target(false)
        .with_ansi(false)
        .compact()
        .try_init();
}
