//! 接続ごとに1要求だけを読む、小さな HTTP/1.1 の処理。

use std::fs::File;
use std::io::{self, BufRead, Read, Write};

pub const MAX_HEADERS: usize = 16 * 1024;
pub const MAX_BODY: u64 = 1024 * 1024;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub head_bytes: usize,
}

impl Request {
    pub fn headers<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.headers.iter().filter(move |(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }
}

pub enum Body {
    Bytes(Vec<u8>),
    File(File, u64),
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

impl Response {
    pub fn bytes(status: u16, content_type: &str, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: vec![
                ("Content-Type".into(), content_type.into()),
                ("X-Content-Type-Options".into(), "nosniff".into()),
            ],
            body: Body::Bytes(bytes.into()),
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn error(status: u16) -> Self {
        let response = Self::bytes(status, "text/plain; charset=utf-8", format!("{}\n", error_message(status)))
            .header("Content-Security-Policy", "sandbox; default-src 'none'")
            .header("Cache-Control", "no-store");
        if status == 405 { response.header("Allow", "GET") } else { response }
    }
}

pub fn error_message(status: u16) -> &'static str {
    match status {
        400 => "要求の形式が不正です",
        403 => "回答の送信に必要なヘッダが一致しません",
        404 => "見つかりません",
        405 => "GET だけを受け付けます",
        408 => "要求の読み取りがタイムアウトしました",
        409 => "確認事項は取り下げ済みです",
        411 => "Content-Length が必要です",
        413 => "本文が大きすぎます",
        421 => "Host が一致しません",
        422 => "回答の内容が不正です",
        431 => "ヘッダが大きすぎます",
        503 => "接続が混み合っています",
        _ => "処理できませんでした",
    }
}

fn token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

pub fn read_head(reader: &mut impl BufRead) -> Result<Request, u16> {
    let mut head = Vec::new();
    loop {
        let start = head.len();
        let read =
            (&mut *reader).take((MAX_HEADERS + 1 - start) as u64).read_until(b'\n', &mut head).map_err(|error| {
                if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) {
                    408u16
                } else {
                    400u16
                }
            })?;
        if head.len() > MAX_HEADERS {
            return Err(431);
        }
        if read == 0 || !head[start..].ends_with(b"\r\n") {
            return Err(400);
        }
        if &head[start..] == b"\r\n" {
            break;
        }
    }
    let text = std::str::from_utf8(&head).map_err(|_| 400u16)?;
    let mut lines = text.split("\r\n");
    let words: Vec<_> = lines.next().ok_or(400u16)?.split(' ').collect();
    let [method, target, version] = words.as_slice() else { return Err(400) };
    if method.is_empty()
        || !method.bytes().all(token)
        || !target.starts_with('/')
        || target.starts_with("//")
        || target.chars().any(|c| c.is_control() || c.is_whitespace() || c == '#')
        || !matches!(*version, "HTTP/1.1" | "HTTP/1.0")
    {
        return Err(400);
    }
    let mut headers = Vec::new();
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or(400u16)?;
        if name.is_empty() || !name.bytes().all(token) || value.bytes().any(|b| b.is_ascii_control() && b != b'\t') {
            return Err(400);
        }
        headers.push((name.to_ascii_lowercase(), value.trim_matches([' ', '\t']).to_string()));
    }
    Ok(Request {
        method: method.to_string(),
        target: target.to_string(),
        headers,
        body: Vec::new(),
        head_bytes: head.len(),
    })
}

pub fn read_body(reader: impl Read, request: &mut Request) -> Result<(), u16> {
    if request.headers("transfer-encoding").next().is_some() {
        return Err(400);
    }
    let lengths: Vec<_> = request.headers("content-length").collect();
    if lengths.len() > 1 {
        return Err(400);
    }
    let length = if let Some(length) = lengths.first() {
        if length.is_empty() || !length.bytes().all(|b| b.is_ascii_digit()) {
            return Err(400);
        }
        let length = length.parse::<u64>().map_err(|_| 413u16)?;
        if length > MAX_BODY.saturating_sub(request.head_bytes as u64) {
            return Err(413);
        }
        Some(length)
    } else {
        None
    };
    if request.method == "POST" && request.target.split('?').next() == Some("/api/answers") {
        let length = length.ok_or(411u16)?;
        reader.take(length).read_to_end(&mut request.body).map_err(|error| {
            if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) { 408u16 } else { 400u16 }
        })?;
        if request.body.len() as u64 != length {
            return Err(400);
        }
    }
    Ok(())
}

#[cfg(test)]
fn read_request(reader: impl Read) -> Result<Request, u16> {
    let mut reader = std::io::BufReader::new(reader);
    let mut request = read_head(&mut reader)?;
    read_body(reader, &mut request)?;
    Ok(request)
}

pub fn write_response(mut writer: impl Write, response: Response) -> io::Result<()> {
    let reason = match response.status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        421 => "Misdirected Request",
        422 => "Unprocessable Content",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let length = match &response.body {
        Body::Bytes(bytes) => bytes.len() as u64,
        Body::File(_, length) => *length,
    };
    write!(writer, "HTTP/1.1 {} {reason}\r\n", response.status)?;
    for (name, value) in response.headers {
        write!(writer, "{name}: {value}\r\n")?;
    }
    write!(writer, "Content-Length: {length}\r\nConnection: close\r\n\r\n")?;
    match response.body {
        Body::Bytes(bytes) => writer.write_all(&bytes)?,
        Body::File(file, length) => {
            io::copy(&mut file.take(length), &mut writer)?;
        }
    }
    writer.flush()
}

/// `%` を1回だけデコードする。`+` はパスではそのまま保つ。
pub fn decode(value: &str) -> Result<String, u16> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = input.next().and_then(|b| (b as char).to_digit(16)).ok_or(400u16)?;
            let low = input.next().and_then(|b| (b as char).to_digit(16)).ok_or(400u16)?;
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).map_err(|_| 400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_limits_headers_and_body_and_rejects_ambiguous_framing() {
        let big = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEADERS));
        assert_eq!(read_request(big.as_bytes()).unwrap_err(), 431);
        for (header, status) in [
            ("Content-Length: 1048577", 413),
            ("Content-Length: 999999999999999999999999999", 413),
            ("Content-Length: -1", 400),
            ("Content-Length: 0\r\nContent-Length: 0", 400),
            ("Transfer-Encoding: chunked", 400),
            (" Host: example", 400),
            ("X: bad\0value", 400),
        ] {
            let raw = format!("POST /api/answers HTTP/1.1\r\n{header}\r\n\r\n");
            assert_eq!(read_request(raw.as_bytes()).unwrap_err(), status, "{header}");
        }
        assert!(read_request(b"GET / HTTP/1.1\r\nHost: localhost:1\r\n\r\n".as_slice()).is_ok());
    }

    #[test]
    fn parser_rejects_broken_request_lines_and_preserves_duplicate_hosts() {
        for line in ["GET /", "GET  / HTTP/1.1", "GET http://example/ HTTP/1.1", "GET /#x HTTP/1.1", "GET / HTTP/2"] {
            assert_eq!(read_request(format!("{line}\r\n\r\n").as_bytes()).unwrap_err(), 400);
        }
        assert_eq!(read_request(b"GET / HTTP/1.1\n\n".as_slice()).unwrap_err(), 400);
        let request = read_request(b"GET / HTTP/1.1\r\nHost: a\r\nhOSt: b\r\n\r\n".as_slice()).unwrap();
        assert_eq!(request.headers("Host").collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn percent_decoding_is_once_and_requires_utf8() {
        assert_eq!(decode("%E8%AA%BF%E6%9F%BB+a").unwrap(), "調査+a");
        assert_eq!(decode("%252e%252e").unwrap(), "%2e%2e");
        for invalid in ["%", "%2", "%GG", "%ff", "%c0%af"] {
            assert_eq!(decode(invalid), Err(400));
        }
    }

    #[test]
    fn response_has_length_close_and_nosniff() {
        let mut bytes = Vec::new();
        write_response(&mut bytes, Response::error(405)).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Connection: close\r\n"));
        assert!(text.contains("X-Content-Type-Options: nosniff\r\n"));
        assert!(text.contains("Allow: GET\r\n"));
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        assert!(head.contains(&format!("Content-Length: {}", body.len())));
    }

    #[test]
    fn reads_exact_content_length_with_buffered_body_and_rejects_short_bodies() {
        let body = "日本語の回答";
        let raw = format!("POST /api/answers HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}余分なデータ", body.len());
        assert_eq!(read_request(raw.as_bytes()).unwrap().body, body.as_bytes());
        assert_eq!(read_request(b"POST /api/answers HTTP/1.1\r\n\r\n".as_slice()).unwrap_err(), 411);
        assert_eq!(
            read_request(b"POST /api/answers HTTP/1.1\r\nContent-Length: 3\r\n\r\n{}".as_slice()).unwrap_err(),
            400
        );
        // 1 MiB の本文だけでなく、要求行とヘッダも全体の上限に含める。
        assert_eq!(
            read_request(b"POST /api/answers HTTP/1.1\r\nContent-Length: 1048576\r\n\r\n".as_slice()).unwrap_err(),
            413
        );
    }

    #[test]
    fn body_timeouts_keep_the_existing_408_status() {
        struct Timeout;
        impl Read for Timeout {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::TimedOut.into())
            }
        }
        let head = b"POST /api/answers HTTP/1.1\r\nContent-Length: 2\r\n\r\n";
        assert_eq!(read_request(head.as_slice().chain(Timeout)).unwrap_err(), 408);
    }
}
