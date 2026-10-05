// SPDX-License-Identifier: GPL-3.0-only
//! A small HTTP/1.1 server on the loopback interface: static files from the
//! UI source (with the Web MIDI shim injected into pages), `/__fm1/info`
//! and the `/midi` WebSocket. One thread per connection, `Connection: close`.
use super::{hub::Hub, source, ws, Source};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

/// Where the GUI serves the editor.
pub const ADDRESS: &str = "127.0.0.1:8765";
const SHIM: &str = include_str!("shim.js");
const MAX_HEAD: usize = 16 * 1024;

pub struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
}

/// The parts of a request this server looks at.
pub(crate) struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

struct Shared {
    source: Option<Source>,
    hub: Arc<Hub>,
    address: SocketAddr,
}

impl Server {
    /// Listen on `address`, which must be a loopback address (port 0 picks
    /// a free port), and serve from a background thread.
    pub fn start(address: &str, source: Option<Source>, hub: Arc<Hub>) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        let address = listener.local_addr()?;
        if !address.ip().is_loopback() {
            return Err(io::Error::other(
                "the editor server only listens on loopback",
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            source,
            hub,
            address,
        });
        let flag = stop.clone();
        thread::Builder::new()
            .name("fm1-web".into())
            .spawn(move || accept(listener, shared, flag))?;
        Ok(Self { address, stop })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn url(&self) -> String {
        format!("http://{}/", self.address)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept so the thread sees the flag.
        let _ = TcpStream::connect(self.address);
    }
}

fn accept(listener: TcpListener, shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = stream else { continue };
        let shared = shared.clone();
        let spawned = thread::Builder::new()
            .name("fm1-web-client".into())
            .spawn(move || {
                if let Err(error) = handle(stream, &shared) {
                    eprintln!("editor server: {error}");
                }
            });
        if let Err(error) = spawned {
            eprintln!("editor server: no thread for a client: {error}");
        }
    }
}

fn handle(mut stream: TcpStream, shared: &Shared) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let Some(request) = read_request(&mut stream)? else {
        return respond(&mut stream, 400, "text/plain", b"bad request", false);
    };
    if !local_host(request.header("Host"), shared.address.port()) {
        return respond(&mut stream, 403, "text/plain", b"forbidden host", false);
    }
    let head = request.method == "HEAD";
    if request.method != "GET" && !head {
        return respond(&mut stream, 405, "text/plain", b"GET only", false);
    }
    let path = request.target.split(['?', '#']).next().unwrap_or("/");
    match path {
        "/midi" => {
            let origin_ok = request
                .header("Origin")
                .is_none_or(|o| local_origin(o, shared.address.port()));
            if !origin_ok {
                return respond(&mut stream, 403, "text/plain", b"forbidden origin", false);
            }
            ws::serve(stream, &request, &shared.hub)
        }
        "/__fm1/info" => {
            let hub = &shared.hub;
            let body = format!(
                "{{\"name\":{},\"ready\":{}}}",
                json_string(&hub.port_name()),
                hub.ready()
            );
            respond(&mut stream, 200, "application/json", body.as_bytes(), head)
        }
        "/__fm1/shim.js" => respond(
            &mut stream,
            200,
            source::mime_type(".js"),
            SHIM.as_bytes(),
            head,
        ),
        _ => static_file(&mut stream, shared, path, head),
    }
}

fn static_file(stream: &mut TcpStream, shared: &Shared, path: &str, head: bool) -> io::Result<()> {
    let Some(source) = &shared.source else {
        let text = "No web editor for this firmware: put X-ui.zip next to X.fwsc, or start fm1-ui with --ui DIR.";
        return respond(
            stream,
            404,
            "text/plain; charset=utf-8",
            text.as_bytes(),
            head,
        );
    };
    let Some(clean) = source::percent_decode(path).and_then(|p| source::clean_path(&p)) else {
        return respond(stream, 400, "text/plain", b"bad path", head);
    };
    let Some(body) = source.get(&clean) else {
        return respond(stream, 404, "text/plain", b"not found", head);
    };
    let mime = source::mime_type(&clean);
    if mime.starts_with("text/html") {
        let page = inject_shim(&String::from_utf8_lossy(&body));
        return respond(stream, 200, mime, page.as_bytes(), head);
    }
    respond(stream, 200, mime, &body, head)
}

/// The page with the shim as the first script of its head.
pub fn inject_shim(page: &str) -> String {
    let lower = page.to_ascii_lowercase();
    let after = |tag: &str| {
        let start = lower.find(tag)?;
        let rest = &lower[start + tag.len()..];
        // "<head>" or "<head attr>", not "<header>"
        if !rest.starts_with(['>', ' ', '\t', '\n', '\r']) {
            return None;
        }
        Some(start + lower[start..].find('>')? + 1)
    };
    let at = after("<head").or_else(|| after("<html")).unwrap_or(0);
    format!("{}<script>{SHIM}</script>{}", &page[..at], &page[at..])
}

fn read_request(stream: &mut TcpStream) -> io::Result<Option<Request>> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 2048];
    while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buffer)?;
        if n == 0 || raw.len() + n > MAX_HEAD {
            return Ok(None);
        }
        raw.extend_from_slice(&buffer[..n]);
    }
    let Ok(text) = std::str::from_utf8(&raw) else {
        return Ok(None);
    };
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (Some(method), Some(target), Some(version)) = (first.next(), first.next(), first.next())
    else {
        return Ok(None);
    };
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Ok(None);
    }
    let headers = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Ok(Some(Request {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
    }))
}

/// Host names a loopback page uses; anything else (DNS rebinding) is refused.
fn local_host(host: Option<&str>, port: u16) -> bool {
    let Some(host) = host else { return false };
    ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|name| host.eq_ignore_ascii_case(&format!("{name}:{port}")))
}

fn local_origin(origin: &str, port: u16) -> bool {
    origin
        .strip_prefix("http://")
        .is_some_and(|host| local_host(Some(host), port))
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    mime: &str,
    body: &[u8],
    head: bool,
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Method Not Allowed",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if !head {
        stream.write_all(body)?;
    }
    stream.flush()
}
