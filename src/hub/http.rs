//! 接続ごとに1要求だけを読む、小さな HTTP/1.1 の処理。

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};

pub const MAX_HEADERS: usize = 16 * 1024;
pub const MAX_BODY: u64 = 1024 * 1024;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
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
        let message = match status {
            400 => "要求の形式が不正です",
            404 => "見つかりません",
            405 => "GET だけを受け付けます",
            408 => "要求の読み取りがタイムアウトしました",
            413 => "本文が大きすぎます",
            421 => "Host が一致しません",
            431 => "ヘッダが大きすぎます",
            503 => "接続が混み合っています",
            _ => "処理できませんでした",
        };
        let response = Self::bytes(status, "text/plain; charset=utf-8", format!("{message}\n"))
            .header("Content-Security-Policy", "sandbox; default-src 'none'")
            .header("Cache-Control", "no-store");
        if status == 405 { response.header("Allow", "GET") } else { response }
    }
}

fn token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

pub fn read_request(reader: impl Read) -> Result<Request, u16> {
    let mut reader = BufReader::new(reader);
    let mut head = Vec::new();
    loop {
        let start = head.len();
        let read =
            (&mut reader).take((MAX_HEADERS + 1 - start) as u64).read_until(b'\n', &mut head).map_err(|error| {
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
    let request = Request { method: method.to_string(), target: target.to_string(), headers };
    if request.headers("transfer-encoding").next().is_some() {
        return Err(400);
    }
    let lengths: Vec<_> = request.headers("content-length").collect();
    if lengths.len() > 1 {
        return Err(400);
    }
    if let Some(length) = lengths.first() {
        if length.is_empty() || !length.bytes().all(|b| b.is_ascii_digit()) {
            return Err(400);
        }
        let length = length.parse::<u64>().map_err(|_| 413u16)?;
        if length > MAX_BODY {
            return Err(413);
        }
    }
    // 読み取り専用のハブでは本文を使わない。POST は経路を作らず 405 で閉じる。
    Ok(request)
}

pub fn write_response(mut writer: impl Write, response: Response) -> io::Result<()> {
    let reason = match response.status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Content Too Large",
        421 => "Misdirected Request",
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
}
