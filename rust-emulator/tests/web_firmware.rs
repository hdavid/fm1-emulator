// SPDX-License-Identifier: GPL-3.0-only
// End to end: a WebSocket client (what the Web MIDI shim is) talks to the
// unchanged firmware through the server, the hub, `web::pump` and the USB
// MIDI host. Set MIDI_FWSC to a Felucca, Jangada or SLOOP package and run
// in release mode with --ignored.
use fm1_emu::{
    cpu::Cpu,
    firmware::Firmware,
    usb_midi::Decoder,
    web::{pump, Hub, Server},
};
use std::{env, net::TcpStream, path::Path, sync::Arc, time::Duration};
use tungstenite::{Message, WebSocket};

fn client(server: &Server) -> WebSocket<TcpStream> {
    let stream = TcpStream::connect(server.address()).unwrap();
    let (socket, _) =
        tungstenite::client(format!("ws://{}/midi", server.address()), stream).unwrap();
    socket
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(1)))
        .unwrap();
    socket
}

/// Run the CPU in slices, pumping MIDI between slices like the GUI does.
fn run(cpu: &mut Cpu, hub: &Hub, decoder: &mut Decoder, slices: u32) {
    for _ in 0..slices {
        pump(hub, &mut cpu.bus, decoder);
        cpu.run_steps(1_000_000).unwrap();
    }
    pump(hub, &mut cpu.bus, decoder);
}

fn frames(socket: &mut WebSocket<TcpStream>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while let Ok(message) = socket.read() {
        if let Message::Binary(bytes) = message {
            out.push(bytes.to_vec());
        }
    }
    out
}

fn peak(cpu: &Cpu) -> u32 {
    let samples = cpu.bus.audio.samples.iter();
    samples
        .flat_map(|f| f.map(i32::unsigned_abs))
        .max()
        .unwrap_or(0)
}

#[test]
#[ignore = "requires MIDI_FWSC; run in release mode with --ignored"]
fn a_websocket_client_plays_the_firmware_and_reads_its_sysex_replies() {
    let path = env::var("MIDI_FWSC").expect("set MIDI_FWSC to a Felucca-family package");
    let firmware = Firmware::load(Path::new(&path)).unwrap();
    let mut bus = firmware.bus().unwrap();
    bus.usb.enable_midi_host();
    let mut cpu = Cpu::new(bus, firmware.entry);
    cpu.r[0] = 0x01c7fe08;
    let hub = Arc::new(Hub::default());
    let server = Server::start("127.0.0.1:0", None, hub.clone()).unwrap();
    let mut socket = client(&server);
    let mut decoder = Decoder::default();
    run(&mut cpu, &hub, &mut decoder, 100);
    assert!(hub.ready(), "the firmware must be enumerated by now");
    assert_eq!(hub.port_name(), "Felucca (FM-1 Emulator)");
    frames(&mut socket);

    // A note-on from the browser: the audio level rises from silence.
    cpu.bus.audio.samples.clear();
    run(&mut cpu, &hub, &mut decoder, 5);
    let before = peak(&cpu);
    socket.send(Message::binary(vec![0x90, 60, 110])).unwrap();
    cpu.bus.audio.samples.clear();
    run(&mut cpu, &hub, &mut decoder, 10);
    let after = peak(&cpu);
    eprintln!("audio peak before {before}, after the note-on {after}");
    assert_eq!(before, 0);
    assert!(after > 500, "peak {after}");
    socket.send(Message::binary(vec![0x80, 60, 0])).unwrap();

    // An editor request (INFO) gets its reply as one WebSocket frame.
    socket
        .send(Message::binary(vec![0xF0, 0x7D, 0x46, 0x4C, 1, 0xF7]))
        .unwrap();
    let mut replies = Vec::new();
    for _ in 0..40 {
        run(&mut cpu, &hub, &mut decoder, 1);
        replies.extend(frames(&mut socket));
        if replies
            .iter()
            .any(|m| m.starts_with(&[0xF0, 0x7D, 0x46, 0x4C, 1]))
        {
            break;
        }
    }
    let info = replies
        .iter()
        .find(|m| m.starts_with(&[0xF0, 0x7D, 0x46, 0x4C, 1]))
        .unwrap_or_else(|| panic!("no INFO reply; frames {replies:02x?}"));
    let text: String = info[5..]
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as char)
        .collect();
    eprintln!("INFO reply: {} bytes, version {text:?}", info.len());
    assert_eq!(info.last(), Some(&0xF7));
    let stats = hub.stats();
    assert_eq!(stats.clients, 1);
    assert!(stats.to_device >= 3 && stats.from_device >= 1, "{stats:?}");
}
