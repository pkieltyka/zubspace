//! Moves log writes off the threads that emit them. Each formatted line is
//! queued for one writer thread. When the queue is full because journald or
//! the log disk has stalled, the line is dropped and counted instead of
//! blocking the emitter: the game tick logs while it holds the state lock.

use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::thread;

/// Minutes of today's heaviest logging (dev at DEBUG, about 140 lines/s)
/// would fit; a full queue means the writer has stopped making progress.
const QUEUE_LINES: usize = 16_384;

static DROPPED_LINES: AtomicU64 = AtomicU64::new(0);

/// Lines dropped since startup because the queue was full.
pub(crate) fn dropped_lines() -> u64 {
    DROPPED_LINES.load(Ordering::Relaxed)
}

#[derive(Clone)]
pub(crate) struct QueuedWriter {
    sender: SyncSender<Vec<u8>>,
    dropped: &'static AtomicU64,
}

impl QueuedWriter {
    /// Starts the writer thread that owns `output`.
    pub(crate) fn spawn(output: impl Write + Send + 'static) -> io::Result<Self> {
        Self::spawn_with(output, QUEUE_LINES, &DROPPED_LINES)
    }

    fn spawn_with(
        mut output: impl Write + Send + 'static,
        capacity: usize,
        dropped: &'static AtomicU64,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(capacity);
        thread::Builder::new()
            .name("log-writer".to_string())
            .spawn(move || {
                while let Ok(line) = receiver.recv() {
                    let _ = output.write_all(&line);
                    // Flush once the backlog is written, not after every line.
                    while let Ok(line) = receiver.try_recv() {
                        let _ = output.write_all(&line);
                    }
                    let _ = output.flush();
                }
            })?;
        Ok(Self { sender, dropped })
    }
}

/// One formatted event, sent to the writer thread when the formatter is done.
pub(crate) struct QueuedLine {
    sender: SyncSender<Vec<u8>>,
    dropped: &'static AtomicU64,
    bytes: Vec<u8>,
}

impl Write for QueuedLine {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for QueuedLine {
    fn drop(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        if self
            .sender
            .try_send(std::mem::take(&mut self.bytes))
            .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for QueuedWriter {
    type Writer = QueuedLine;

    fn make_writer(&'a self) -> QueuedLine {
        QueuedLine {
            sender: self.sender.clone(),
            dropped: self.dropped,
            bytes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tracing_subscriber::fmt::MakeWriter;

    /// Writes nothing until the test opens the gate, like a stalled journald.
    struct Gated {
        gate: Receiver<()>,
        written: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for Gated {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let _ = self.gate.recv();
            self.written.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn emit(writer: &QueuedWriter, text: &str) {
        let mut line = writer.make_writer();
        line.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn a_stalled_writer_drops_and_counts_lines_without_blocking_the_emitter() {
        static DROPPED: AtomicU64 = AtomicU64::new(0);
        let (open, gate) = mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let writer = QueuedWriter::spawn_with(
            Gated {
                gate,
                written: written.clone(),
            },
            2,
            &DROPPED,
        )
        .unwrap();

        let started = Instant::now();
        for index in 0..10 {
            emit(&writer, &format!("line {index}\n"));
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        let dropped = DROPPED.load(Ordering::Relaxed);
        // The queue holds two lines and the writer thread at most one more.
        assert!((7..=8).contains(&dropped), "dropped {dropped}");

        for _ in 0..10 {
            let _ = open.send(());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let expected_lines = 10 - dropped as usize;
        loop {
            let text = String::from_utf8(written.lock().unwrap().clone()).unwrap();
            if text.lines().count() == expected_lines {
                // Kept lines arrive whole and in emission order.
                let indexes: Vec<usize> = text
                    .lines()
                    .map(|line| line.strip_prefix("line ").unwrap().parse().unwrap())
                    .collect();
                assert!(indexes.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
                break;
            }
            assert!(Instant::now() < deadline, "only wrote {text:?}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn every_line_is_written_when_the_writer_keeps_up() {
        static DROPPED: AtomicU64 = AtomicU64::new(0);
        let (open, gate) = mpsc::channel();
        for _ in 0..100 {
            open.send(()).unwrap();
        }
        let written = Arc::new(Mutex::new(Vec::new()));
        let writer = QueuedWriter::spawn_with(
            Gated {
                gate,
                written: written.clone(),
            },
            128,
            &DROPPED,
        )
        .unwrap();
        for index in 0..100 {
            emit(&writer, &format!("line {index}\n"));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while written
            .lock()
            .unwrap()
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count()
            < 100
        {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(DROPPED.load(Ordering::Relaxed), 0);
    }
}
