//! Immutable identity of the running executable, captured before accepting
//! clients. Never hash a mutable deployment pathname during a QA marker.
use std::io::{self, Read};
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

const MAX_EXECUTABLE_BYTES: u64 = 1024 * 1024 * 1024;
static RUNNING_IDENTITY: OnceLock<Option<BinaryIdentity>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BinaryIdentity {
    pub(crate) sha256: [u8; 32],
    pub(crate) bytes: u64,
}

fn hash_reader(mut reader: impl Read, limit: u64) -> io::Result<BinaryIdentity> {
    let mut sha256 = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "executable exceeds identity limit",
            ));
        }
        sha256.update(&buffer[..count]);
    }
    if bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty executable",
        ));
    }
    Ok(BinaryIdentity {
        sha256: sha256.finalize().into(),
        bytes,
    })
}

#[cfg(target_os = "linux")]
fn running_executable_identity() -> io::Result<BinaryIdentity> {
    // procfs refers to this process's executable inode even after an atomic
    // deployment replacement/unlink. Linux excludes writes to a running
    // executable; the open descriptor cannot silently follow a new pathname.
    hash_reader(std::fs::File::open("/proc/self/exe")?, MAX_EXECUTABLE_BYTES)
}

#[cfg(not(target_os = "linux"))]
fn running_executable_identity() -> io::Result<BinaryIdentity> {
    // A portable current_exe pathname does not prove which inode is running
    // after replacement. Report the unsupported tier instead of inventing
    // an exact identity. The deployed server target is Linux.
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "running executable identity unavailable",
    ))
}

pub(crate) fn initialize() {
    RUNNING_IDENTITY.get_or_init(|| match running_executable_identity() {
        Ok(identity) => Some(identity),
        Err(_) => {
            tracing::warn!("QA running-server binary identity unavailable");
            None
        }
    });
}

pub(crate) fn captured() -> Option<BinaryIdentity> {
    RUNNING_IDENTITY.get().copied().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_hash_is_exact_bounded_and_rejects_empty_or_failed_reads() {
        let identity = hash_reader(&b"abc"[..], 3).unwrap();
        assert_eq!(identity.bytes, 3);
        assert_eq!(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            identity
                .sha256
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        assert!(hash_reader(&b"abcd"[..], 3).is_err());
        assert!(hash_reader(&b""[..], 3).is_err());
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("injected read failure"))
            }
        }
        assert!(hash_reader(Failed, 3).is_err());
    }

    #[test]
    fn identity_hash_consumes_multiple_chunks_exactly() {
        let bytes = vec![0xa5; 3 * 64 * 1024 + 17];
        let identity = hash_reader(bytes.as_slice(), bytes.len() as u64).unwrap();
        assert_eq!(identity.sha256, <[u8; 32]>::from(Sha256::digest(&bytes)));
        assert_eq!(identity.bytes, bytes.len() as u64);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn running_executable_identity_child() {
        if std::env::var("ZUBSPACE_TEST_BINARY_IDENTITY_CHILD").as_deref() != Ok("1") {
            return;
        }
        use std::io::Write;
        initialize();
        let before = captured().expect("running child identity");
        // Serial libtest prints its progress prefix without a newline. Own
        // the framing so the parent sees the same protocol on one or many CPUs.
        println!("\nbinary_identity_ready");
        std::io::stdout().flush().unwrap();
        let mut command = String::new();
        std::io::stdin().read_line(&mut command).unwrap();
        assert_eq!(command.trim(), "replaced");
        assert_eq!(captured(), Some(before));
        assert_eq!(
            running_executable_identity().unwrap(),
            before,
            "the running inode, not a replacement pathname, owns this identity"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn running_identity_survives_deployment_path_unlink_and_replacement() {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        struct TemporaryDirectory(std::path::PathBuf);
        impl Drop for TemporaryDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory_path = std::env::temp_dir().join(format!(
            "zubspace-binary-identity-{}-{}",
            std::process::id(),
            u64::from_le_bytes(random)
        ));
        std::fs::create_dir(&directory_path).unwrap();
        let directory = TemporaryDirectory(directory_path);
        let executable = directory.0.join("running-test");
        std::fs::copy("/proc/self/exe", &executable).unwrap();
        let mut child = ChildGuard(
            Command::new(&executable)
                .args([
                    "--exact",
                    "binary_identity::tests::running_executable_identity_child",
                    "--nocapture",
                ])
                .env("ZUBSPACE_TEST_BINARY_IDENTITY_CHILD", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let output = child.0.stdout.take().unwrap();
        let (output_tx, output_rx) = std::sync::mpsc::channel();
        let output_task = std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if output_tx.send(line).is_err() {
                    break;
                }
            }
        });
        let ready_deadline = Instant::now() + Duration::from_secs(30);
        let mut transcript = String::new();
        loop {
            let line = output_rx
                .recv_timeout(ready_deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| {
                    panic!("identity child readiness failed: {error}; stdout: {transcript}")
                })
                .expect("identity child stdout read");
            transcript.push_str(&line);
            transcript.push('\n');
            if line.trim() == "binary_identity_ready" {
                break;
            }
        }
        // Only this test-created copy is removed. The running inode remains
        // readable through procfs even though its former name now has bytes
        // that are deliberately not an executable.
        std::fs::remove_file(&executable).unwrap();
        std::fs::write(&executable, b"replacement deployment").unwrap();
        child
            .0
            .stdin
            .take()
            .unwrap()
            .write_all(b"replaced\n")
            .unwrap();
        let exit_deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < exit_deadline,
                "identity child did not exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        output_task.join().expect("identity child stdout reader");
        for line in output_rx {
            transcript.push_str(&line.expect("identity child stdout read"));
            transcript.push('\n');
        }
        assert!(status.success(), "{transcript}");
    }
}
