//! The status of every HTTP/1.x response on a connection, read from the bytes as they pass. A
//! connection whose first client bytes are not an HTTP method (TLS, HTTP/2, a database protocol)
//! is left alone. Requests are framed as well as responses: a response to `HEAD` carries a
//! `Content-Length` but no body, so where the next response starts depends on the request.

use std::collections::VecDeque;

const METHODS: [&[u8]; 9] = [
    b"GET", b"POST", b"PUT", b"HEAD", b"DELETE", b"PATCH", b"OPTIONS", b"TRACE", b"CONNECT",
];
/// `OPTIONS ` and `CONNECT ` are the longest starts.
const START_LIMIT: usize = 8;
/// A head or a chunk-size line longer than this is not followed.
const LINE_LIMIT: usize = 64 * 1024;

#[derive(Default)]
pub struct HttpSniff {
    kind: Kind,
    start: Vec<u8>,
    requests: Framer,
    responses: Framer,
    /// The requests not answered yet, oldest first.
    asked: VecDeque<Asked>,
}

#[derive(Default, PartialEq)]
enum Kind {
    #[default]
    Undecided,
    Http,
    Other,
}

#[derive(Clone, Copy, PartialEq)]
enum Asked {
    Head,
    Connect,
    Other,
}

impl HttpSniff {
    pub fn feed_request(&mut self, bytes: &[u8]) {
        if self.kind == Kind::Undecided {
            self.kind = self.decide(bytes);
        }
        if self.kind != Kind::Other {
            let asked = &mut self.asked;
            self.requests.feed(bytes, |head| request_body(head, asked));
        }
    }

    /// The statuses of the responses whose heads end in `bytes`.
    pub fn feed_response(&mut self, bytes: &[u8]) -> Vec<u16> {
        let mut statuses = Vec::new();
        if self.kind == Kind::Http {
            let asked = &mut self.asked;
            self.responses
                .feed(bytes, |head| response_body(head, asked, &mut statuses));
        }
        statuses
    }

    pub fn is_http(&self) -> bool {
        self.kind == Kind::Http
    }

    fn decide(&mut self, bytes: &[u8]) -> Kind {
        let room = START_LIMIT - self.start.len();
        self.start
            .extend_from_slice(&bytes[..bytes.len().min(room)]);
        match self.start.iter().position(|&byte| byte == b' ') {
            Some(end) if is_method(&self.start[..end]) => Kind::Http,
            Some(_) => Kind::Other,
            None if METHODS.iter().any(|method| method.starts_with(&self.start)) => Kind::Undecided,
            None => Kind::Other,
        }
    }
}

/// Where one direction of the connection is.
enum At {
    Head(Vec<u8>),
    Body(u64),
    ChunkSize(Vec<u8>),
    /// The chunk's bytes and the CRLF after them.
    ChunkData(u64),
    Trailers(Vec<u8>),
    /// A body that ends only with the connection, a tunnel, or bytes that are not HTTP.
    Lost,
}

struct Framer(At);

impl Default for Framer {
    fn default() -> Framer {
        Framer(At::Head(Vec::new()))
    }
}

impl Framer {
    /// Moves through `bytes`; `head` gets each complete head and says what follows it.
    fn feed(&mut self, mut bytes: &[u8], mut head: impl FnMut(&[u8]) -> At) {
        while !bytes.is_empty() {
            self.0 = match std::mem::replace(&mut self.0, At::Lost) {
                At::Head(mut line) => match fill(&mut line, &mut bytes, b"\r\n\r\n") {
                    Fill::Done => head(&line),
                    Fill::More => At::Head(line),
                    Fill::TooLong => At::Lost,
                },
                At::Body(left) => match skip(left, &mut bytes) {
                    0 => At::Head(Vec::new()),
                    left => At::Body(left),
                },
                At::ChunkSize(mut line) => match fill(&mut line, &mut bytes, b"\n") {
                    Fill::Done => match chunk_size(&line) {
                        // The CRLF that ends the size line also stands for the first one of the
                        // trailers' end, so both "0\r\n\r\n" and "0\r\nX: y\r\n\r\n" end the body.
                        Some(0) => At::Trailers(b"\r\n".to_vec()),
                        Some(size) => At::ChunkData(size.saturating_add(2)),
                        None => At::Lost,
                    },
                    Fill::More => At::ChunkSize(line),
                    Fill::TooLong => At::Lost,
                },
                At::ChunkData(left) => match skip(left, &mut bytes) {
                    0 => At::ChunkSize(Vec::new()),
                    left => At::ChunkData(left),
                },
                At::Trailers(mut line) => match fill(&mut line, &mut bytes, b"\r\n\r\n") {
                    Fill::Done => At::Head(Vec::new()),
                    Fill::More => At::Trailers(line),
                    Fill::TooLong => At::Lost,
                },
                At::Lost => {
                    bytes = &[];
                    At::Lost
                }
            };
        }
    }
}

enum Fill {
    Done,
    More,
    TooLong,
}

/// Moves bytes into `line` until it ends with `end`.
fn fill(line: &mut Vec<u8>, bytes: &mut &[u8], end: &[u8]) -> Fill {
    while let Some((&byte, rest)) = bytes.split_first() {
        *bytes = rest;
        line.push(byte);
        if line.ends_with(end) {
            return Fill::Done;
        }
        if line.len() > LINE_LIMIT {
            return Fill::TooLong;
        }
    }
    Fill::More
}

/// Drops up to `left` bytes; returns how many are still to come.
fn skip(left: u64, bytes: &mut &[u8]) -> u64 {
    let taken = left.min(bytes.len() as u64);
    *bytes = &bytes[taken as usize..];
    left - taken
}

fn request_body(head: &[u8], asked: &mut VecDeque<Asked>) -> At {
    let method = start_line(head)
        .split(|&byte| byte == b' ')
        .next()
        .unwrap_or_default();
    if !is_method(method) {
        return At::Lost;
    }
    let request = match method {
        b"HEAD" => Asked::Head,
        b"CONNECT" => Asked::Connect,
        _ => Asked::Other,
    };
    asked.push_back(request);
    if request == Asked::Connect {
        return At::Lost;
    }
    framing(head, At::Head(Vec::new()))
}

fn response_body(head: &[u8], asked: &mut VecDeque<Asked>, statuses: &mut Vec<u16>) -> At {
    let Some(status) = status(start_line(head)) else {
        return At::Lost;
    };
    statuses.push(status);
    match status {
        101 => return At::Lost,
        // An interim response: the final one to the same request follows.
        100..=199 => return At::Head(Vec::new()),
        _ => {}
    }
    match asked.pop_front() {
        Some(Asked::Head) => At::Head(Vec::new()),
        Some(Asked::Connect) if status < 300 => At::Lost,
        _ if status == 204 || status == 304 => At::Head(Vec::new()),
        _ => framing(head, At::Lost),
    }
}

/// What follows `head` by its `Transfer-Encoding` and `Content-Length`; `unframed` when it has neither.
fn framing(head: &[u8], unframed: At) -> At {
    if let Some(codings) = header(head, b"transfer-encoding") {
        let last = codings
            .rsplit(|&byte| byte == b',')
            .next()
            .unwrap_or_default();
        return if last.trim_ascii().eq_ignore_ascii_case(b"chunked") {
            At::ChunkSize(Vec::new())
        } else {
            At::Lost
        };
    }
    match header(head, b"content-length") {
        Some(length) => std::str::from_utf8(length)
            .ok()
            .and_then(|text| text.parse().ok())
            .map_or(At::Lost, At::Body),
        None => unframed,
    }
}

fn start_line(head: &[u8]) -> &[u8] {
    lines(head.trim_ascii_start()).next().unwrap_or_default()
}

fn header<'a>(head: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    lines(head).skip(1).find_map(|line| {
        let colon = line.iter().position(|&byte| byte == b':')?;
        line[..colon]
            .eq_ignore_ascii_case(name)
            .then(|| line[colon + 1..].trim_ascii())
    })
}

fn lines(head: &[u8]) -> impl Iterator<Item = &[u8]> {
    head.split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
}

/// `HTTP/1.1 204 No Content` -> 204.
fn status(line: &[u8]) -> Option<u16> {
    let mut parts = line.splitn(3, |&byte| byte == b' ');
    if !parts.next()?.starts_with(b"HTTP/1.") {
        return None;
    }
    let status: u16 = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
    (100..=599).contains(&status).then_some(status)
}

/// `5` or `5;name=value`, in hex.
fn chunk_size(line: &[u8]) -> Option<u64> {
    let size = line.split(|&byte| byte == b';').next()?.trim_ascii();
    u64::from_str_radix(std::str::from_utf8(size).ok()?, 16).ok()
}

fn is_method(token: &[u8]) -> bool {
    METHODS.contains(&token)
}

#[cfg(test)]
mod tests {
    use super::HttpSniff;

    const REQUESTS: &[u8] = b"GET / HTTP/1.1\r\nHost: api\r\n\r\n\
        POST /orders HTTP/1.1\r\nContent-Length: 5\r\nExpect: 100-continue\r\n\r\nhello\
        HEAD / HTTP/1.1\r\n\r\n\
        POST /upload HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n\
        GET /gone HTTP/1.1\r\n\r\n";

    const RESPONSES: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok\
        HTTP/1.1 100 Continue\r\n\r\n\
        HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\n\r\n2;x=y\r\nok\r\n0\r\nX-Done: 1\r\n\r\n\
        HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n\
        HTTP/1.1 204 No Content\r\n\r\n\
        HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n";

    fn sniff(requests: &[u8], responses: &[u8], chunk: usize) -> (bool, Vec<u16>) {
        let mut sniff = HttpSniff::default();
        for part in requests.chunks(chunk) {
            sniff.feed_request(part);
        }
        let statuses = responses
            .chunks(chunk)
            .flat_map(|part| sniff.feed_response(part))
            .collect();
        (sniff.is_http(), statuses)
    }

    #[test]
    fn every_response_on_a_kept_alive_connection_is_read() {
        for chunk in [usize::MAX, 7, 1] {
            let read = sniff(REQUESTS, RESPONSES, chunk);
            assert_eq!(
                read,
                (true, vec![200, 100, 201, 200, 204, 503]),
                "in chunks of {chunk}"
            );
        }
    }

    #[test]
    fn a_connection_that_does_not_start_with_a_method_is_left_alone() {
        let starts: [&[u8]; 3] = [
            b"\x16\x03\x01\x02\x00\x01\x00",
            b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n",
            b"GETS / HTTP/1.1\r\n\r\n",
        ];
        for start in starts {
            assert_eq!(
                sniff(start, b"HTTP/1.1 200 OK\r\n\r\n", 1),
                (false, vec![]),
                "{start:?}"
            );
        }
    }
}
