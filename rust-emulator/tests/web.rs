// SPDX-License-Identifier: GPL-3.0-only
#![cfg(feature = "web")]
// The web editor bridge: static files from a UI sidecar (zip or directory),
// the Web MIDI shim injected into pages, and MIDI over the /midi WebSocket.
use fm1_emu::web::{sidecar_for, Hub, Server, Source};
use std::{
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tungstenite::Message;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fm1-web-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (name, data) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

const PAGE: &[u8] = b"<!doctype html><html><head><title>Ed</title></head><body>hi</body></html>";

fn start(source: Option<Source>) -> (Server, Arc<Hub>) {
    let hub = Arc::new(Hub::default());
    let server = Server::start("127.0.0.1:0", source, hub.clone()).unwrap();
    (server, hub)
}

/// One HTTP/1.1 GET: (status, headers lower-cased, body).
fn get(server: &Server, path: &str) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(server.address()).unwrap();
    let host = server.address();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).to_lowercase();
    let status = head[9..12].parse().unwrap();
    (status, head, raw[split + 4..].to_vec())
}

fn zip_source(dir: &Path) -> Source {
    let zip = dir.join("x-ui.zip");
    write_zip(
        &zip,
        &[
            ("index.html", PAGE),
            ("fonts/a.ttf", b"TTF"),
            ("app.js", b"console.log(1)"),
            ("style.css", b"body{}"),
        ],
    );
    Source::open(&zip).unwrap()
}

#[test]
fn the_sidecar_of_x_fwsc_is_x_ui_zip_next_to_it() {
    let dir = scratch("sidecar");
    let firmware = dir.join("felucca-0.9-beta.fwsc");
    std::fs::write(&firmware, b"").unwrap();
    assert_eq!(sidecar_for(&firmware), None);
    write_zip(
        &dir.join("felucca-0.9-beta-ui.zip"),
        &[("index.html", PAGE)],
    );
    assert_eq!(
        sidecar_for(&firmware),
        Some(dir.join("felucca-0.9-beta-ui.zip"))
    );
}

#[test]
fn zip_files_are_served_with_their_mime_types() {
    let dir = scratch("mime");
    let (server, _) = start(Some(zip_source(&dir)));
    let (status, head, body) = get(&server, "/fonts/a.ttf");
    assert_eq!((status, body.as_slice()), (200, &b"TTF"[..]));
    assert!(head.contains("content-type: font/ttf"), "{head}");
    let (_, head, _) = get(&server, "/app.js");
    assert!(head.contains("content-type: text/javascript"), "{head}");
    let (_, head, _) = get(&server, "/style.css?v=2");
    assert!(head.contains("content-type: text/css"), "{head}");
    assert_eq!(get(&server, "/missing.png").0, 404);
}

#[test]
fn html_pages_get_the_web_midi_shim_before_their_own_scripts() {
    let dir = scratch("shim");
    let (server, _) = start(Some(zip_source(&dir)));
    let (status, head, body) = get(&server, "/");
    assert_eq!(status, 200);
    assert!(head.contains("content-type: text/html"), "{head}");
    let page = String::from_utf8(body).unwrap();
    let shim = page.find("requestMIDIAccess").expect("shim injected");
    assert!(shim > page.find("<head>").unwrap());
    assert!(shim < page.find("<title>").unwrap());
    assert!(page.ends_with("<body>hi</body></html>"));
    assert_eq!(get(&server, "/index.html").2, page.as_bytes());
}

#[test]
fn path_traversal_and_foreign_hosts_are_rejected() {
    let dir = scratch("traversal");
    std::fs::write(dir.join("secret.txt"), b"secret").unwrap();
    let ui = dir.join("ui");
    std::fs::create_dir_all(&ui).unwrap();
    std::fs::write(ui.join("index.html"), PAGE).unwrap();
    let (server, _) = start(Some(Source::open(&ui).unwrap()));
    assert_eq!(get(&server, "/index.html").0, 200);
    for path in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/..%2fsecret.txt",
        "/sub/../../secret.txt",
        "/..\\secret.txt",
        "//etc/passwd",
        "/%00",
    ] {
        let (status, _, body) = get(&server, path);
        assert!(status == 400 || status == 404, "{path}: {status}");
        assert!(!body.windows(6).any(|w| w == b"secret"), "{path}");
    }
    // DNS rebinding: a page on another host name must not reach the bridge.
    let mut stream = TcpStream::connect(server.address()).unwrap();
    write!(stream, "GET / HTTP/1.1\r\nHost: evil.example\r\n\r\n").unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 403"), "{raw}");
}

#[test]
fn a_felucca_web_folder_is_served_from_its_editor_html() {
    let dir = scratch("editor");
    std::fs::write(dir.join("editor.html"), PAGE).unwrap();
    let source = Source::open(&dir).unwrap();
    assert!(source.has_index());
    let (server, _) = start(Some(source));
    let (status, _, body) = get(&server, "/");
    assert_eq!(status, 200);
    assert!(String::from_utf8(body)
        .unwrap()
        .contains("requestMIDIAccess"));
}

#[test]
fn without_a_sidecar_the_server_explains_itself() {
    let (server, _) = start(None);
    let (status, _, body) = get(&server, "/");
    assert_eq!(status, 404);
    assert!(String::from_utf8_lossy(&body).contains("-ui.zip"));
}

#[test]
fn info_names_the_port_after_the_usb_product() {
    let (server, hub) = start(None);
    let (_, head, body) = get(&server, "/__fm1/info");
    assert!(head.contains("application/json"));
    assert!(String::from_utf8_lossy(&body).contains("\"name\":\"FM-1 Emulator\""));
    hub.set_device(Some("Felucca"), true);
    let body = String::from_utf8(get(&server, "/__fm1/info").2).unwrap();
    assert_eq!(body, r#"{"name":"Felucca (FM-1 Emulator)","ready":true}"#);
}

fn connect(server: &Server, origin: &str) -> tungstenite::WebSocket<TcpStream> {
    let stream = TcpStream::connect(server.address()).unwrap();
    let request = tungstenite::http::Request::builder()
        .uri(format!("ws://{}/midi", server.address()))
        .header("Host", server.address().to_string())
        .header("Origin", origin)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .unwrap();
    let (socket, _) = tungstenite::client(request, stream).unwrap();
    socket
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(start.elapsed() < Duration::from_secs(5), "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn websocket_midi_round_trips_through_the_hub() {
    let (server, hub) = start(None);
    let origin = format!("http://{}", server.address());
    let mut a = connect(&server, &origin);
    let mut b = connect(&server, &origin);
    wait_for(|| hub.stats().clients == 2);
    // browser -> device: raw bytes, split anywhere, running status allowed
    a.send(Message::binary(vec![0x90, 60, 100, 62, 90, 0xF0, 0x7D]))
        .unwrap();
    a.send(Message::binary(vec![0x01, 0xF7])).unwrap();
    let mut got = Vec::new();
    wait_for(|| {
        got.extend(hub.take_to_device());
        got.len() == 3
    });
    assert_eq!(
        got,
        vec![
            vec![0x90, 60, 100],
            vec![0x90, 62, 90],
            vec![0xF0, 0x7D, 0x01, 0xF7]
        ]
    );
    // device -> browser: one complete message per frame, to every client
    hub.broadcast(&[0xF0, 0x7D, 0x46, 0x4C, 1, 0x30, 0xF7]);
    for socket in [&mut a, &mut b] {
        let message = socket.read().unwrap();
        assert_eq!(
            message.into_data().as_ref(),
            [0xF0, 0x7D, 0x46, 0x4C, 1, 0x30, 0xF7]
        );
    }
    let stats = hub.stats();
    assert_eq!((stats.to_device, stats.from_device), (3, 1));
    drop(a);
    b.close(None).unwrap();
    wait_for(|| hub.stats().clients == 0);
}

#[test]
fn websockets_from_other_origins_are_refused() {
    let (server, hub) = start(None);
    let stream = TcpStream::connect(server.address()).unwrap();
    let request = tungstenite::http::Request::builder()
        .uri(format!("ws://{}/midi", server.address()))
        .header("Host", server.address().to_string())
        .header("Origin", "https://evil.example")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .unwrap();
    assert!(tungstenite::client(request, stream).is_err());
    assert_eq!(hub.stats().clients, 0);
}
