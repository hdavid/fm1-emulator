// SPDX-License-Identifier: GPL-3.0-only
//! The `/midi` WebSocket: binary frames of raw MIDI bytes. From the browser
//! any split is accepted (a running-status parser makes messages of it);
//! to the browser each frame is one complete message.
use super::{hub::Hub, server::Request};
use crate::usb_midi::Parser;
use std::{
    io::{self, ErrorKind, Write},
    net::TcpStream,
    time::Duration,
};
use tungstenite::{handshake::derive_accept_key, protocol::Role, Message, WebSocket};

/// How long a read waits before the session sends what the device said.
const POLL: Duration = Duration::from_millis(5);

pub(crate) fn serve(mut stream: TcpStream, request: &Request, hub: &Hub) -> io::Result<()> {
    let upgrade = request
        .header("Upgrade")
        .is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    let Some(key) = request.header("Sec-WebSocket-Key").filter(|_| upgrade) else {
        let body = b"WebSocket upgrade expected";
        write!(
            stream,
            "HTTP/1.1 426 Upgrade Required\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )?;
        return stream.write_all(body);
    };
    write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        derive_accept_key(key.as_bytes())
    )?;
    stream.set_read_timeout(Some(POLL))?;
    let mut socket = WebSocket::from_raw_socket(stream, Role::Server, None);
    let (id, from_device) = hub.connect();
    let result = session(&mut socket, hub, &from_device);
    hub.disconnect(id);
    result
}

fn session(
    socket: &mut WebSocket<TcpStream>,
    hub: &Hub,
    from_device: &std::sync::mpsc::Receiver<Vec<u8>>,
) -> io::Result<()> {
    let mut parser = Parser::default();
    loop {
        match socket.read() {
            Ok(Message::Binary(bytes)) => {
                for message in bytes.iter().filter_map(|b| parser.push(*b)) {
                    hub.to_device(message);
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {} // text, ping (answered by tungstenite), pong
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ok(())
            }
            // A tab closed without the closing handshake: a normal end here.
            Err(tungstenite::Error::Protocol(
                tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
            )) => return Ok(()),
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionAborted
                        | ErrorKind::BrokenPipe
                        | ErrorKind::UnexpectedEof
                ) =>
            {
                return Ok(())
            }
            Err(error) => return Err(io::Error::other(error)),
        }
        while let Ok(message) = from_device.try_recv() {
            socket
                .send(Message::binary(message))
                .map_err(io::Error::other)?;
        }
    }
}
