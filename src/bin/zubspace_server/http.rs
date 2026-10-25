use std::io::{self, ErrorKind, Read};
use std::net::TcpStream;

pub(crate) fn read_http_header(stream: &mut TcpStream) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    let mut byte = [0; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() > 8192 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "HTTP header is too large",
            ));
        }
        if let Err(error) = stream.read_exact(&mut byte) {
            if error.kind() == ErrorKind::UnexpectedEof {
                if bytes.is_empty() {
                    return Ok(None);
                }
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "HTTP header ended before its terminator",
                ));
            }
            return Err(error);
        }
        bytes.push(byte[0]);
    }

    String::from_utf8(bytes)
        .map(Some)
        .map_err(|err| io::Error::new(ErrorKind::InvalidData, err))
}
