//! Verifies, through the real editor MIDI protocol, whether enabling Mixer
//! Control via MIDI lets the physical MIDI IN/OUT (DIN/TRS) jack control the
//! mixer the way the USB Mixer Control Port does.
//!
//! The scenario: run the editor's own session on cable 2 (identity, global
//! setting dump, then a `mixerControlViaMidi` parameter change) to turn the
//! setting on exactly as the real application would, connect to the DIN
//! jack's host MIDI port (now hotpluggable independently of USB), send a
//! fader-style Control Change into it, and check the firmware-driven
//! channel-1 GPIO LED ring. A known-good control path (the same CC on the
//! USB Mixer Control Port) must move the ring, or the check itself is
//! broken. A second control confirms the setting really took effect:
//! `midi_out_mode`/`mixerControlViaMidi` gates the DIN OUT echo added in
//! `l6max.c`'s LPUART3 fix, so a panel mute press must stop producing that
//! echo once the setting is on.
//!
//! L6 stock firmware maps DIN CCs into Main's event queue, where the native
//! class-2 handler applies them through the mixer setters. Its trace mode
//! verifies the complete receive, dispatch, and mixer-state path. L6max's
//! stock DIN handler remains a factory-jig path; the `din-mixer-control`
//! patch adds the documented mixer routing there. Set
//! `L6_DIN_TRACE_DISPATCH=1` to inspect L6 event-queue and setter calls, or
//! `L6_DIN_USB_FORWARDING=1` to test stock DIN-IN to USB cable-0 forwarding.

#[cfg(target_os = "macos")]
mod imp {
    use coremidi::{
        Client, Destination, Destinations, InputPort, OutputPort, PacketBuffer, Source,
    };
    use l6max_diagnostics::gdb::Gdb;
    use l6max_host::{
        engine::Options,
        headless::Headless,
        host_midi::{din_port_name_for_model, endpoint_name, port_names_for_model},
        indicators,
    };
    use std::time::Duration;
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc::{self, Receiver},
        },
        thread,
    };

    struct Port {
        output: OutputPort,
        destination: Destination,
        #[allow(dead_code)]
        input: InputPort,
        #[allow(dead_code)]
        source: Source,
        rx: Receiver<Vec<u8>>,
    }

    const L6_MAIN_INIT_ADDRESSES: [u32; 10] = [
        0x8001b2f8, 0x800328f8, 0x8001b0e8, 0x8002f8a0, 0x80008bc0, 0x80009a40, 0x80034de8,
        0x80009848, 0x8003bc30, 0x800402c0,
    ];
    impl Port {
        fn send(&self, bytes: &[u8]) -> Result<(), String> {
            self.output
                .send(&self.destination, &PacketBuffer::new(0, bytes))
                .map_err(|e| format!("CoreMIDI send error {e}"))
        }
        fn recv(&self, timeout: Duration) -> Option<Vec<u8>> {
            self.rx.recv_timeout(timeout).ok()
        }
        fn drain(&self) -> Vec<Vec<u8>> {
            let mut out = Vec::new();
            while let Ok(m) = self.rx.try_recv() {
                out.push(m);
            }
            out
        }
    }
    fn open_port(client: &Client, name: &str, tag: &str) -> Result<Port, String> {
        let source = Source::from_name(name).ok_or(format!("missing native MIDI source {name}"))?;
        let destination = Destinations
            .into_iter()
            .find(|d| d.name().as_deref() == Some(name))
            .ok_or(format!("missing native MIDI destination {name}"))?;
        let (tx, rx) = mpsc::channel();
        #[allow(deprecated)]
        let input = client
            .input_port(&format!("{tag} input"), move |packets| {
                for p in packets.iter() {
                    let _ = tx.send(p.data().to_vec());
                }
            })
            .map_err(|e| format!("CoreMIDI input_port error {e}"))?;
        input
            .connect_source(&source)
            .map_err(|e| format!("CoreMIDI connect_source error {e}"))?;
        let output = client
            .output_port(&format!("{tag} output"))
            .map_err(|e| format!("CoreMIDI output_port error {e}"))?;
        Ok(Port {
            output,
            destination,
            input,
            source,
            rx,
        })
    }

    fn l6_event_queue_items(guest: &Headless) -> Result<u32, String> {
        let handle = guest.engine.management[0]
            .read_memory(0, 0x801f8f28, 4)
            .map_err(|error| format!("cannot read L6 event queue handle: {error}"))?;
        let handle: [u8; 4] = handle
            .as_slice()
            .try_into()
            .map_err(|_| "invalid L6 event queue handle read".to_owned())?;
        let handle = u32::from_le_bytes(handle);
        if !(0x8000_0000..0x8100_0000).contains(&handle) {
            return Err(format!("unexpected L6 event queue handle {handle:#x}"));
        }
        let bytes = guest.engine.management[0]
            .read_memory(0, handle + 0x38, 4)
            .map_err(|error| format!("cannot read L6 waiting event count: {error}"))?;
        let bytes: [u8; 4] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| "invalid L6 waiting event count read".to_owned())?;
        Ok(u32::from_le_bytes(bytes))
    }

    type L6MixerSnapshot = (Vec<u8>, Vec<u8>);

    fn l6_mixer_snapshot(guest: &Headless) -> Result<L6MixerSnapshot, String> {
        Ok((
            guest.engine.management[0]
                .read_memory(0, 0x80440dca, 0x7e)
                .map_err(|error| format!("cannot snapshot L6 channel mixer state: {error}"))?,
            guest.engine.management[0]
                .read_memory(0, 0x80440e9a, 6)
                .map_err(|error| format!("cannot snapshot L6 LEVEL state: {error}"))?,
        ))
    }

    fn l6_mixer_changes(before: &L6MixerSnapshot, after: &L6MixerSnapshot) -> Vec<(u32, u8, u8)> {
        [
            (0x80440dca, &before.0, &after.0),
            (0x80440e9a, &before.1, &after.1),
        ]
        .into_iter()
        .flat_map(|(base, before, after)| {
            before
                .iter()
                .zip(after)
                .enumerate()
                .filter_map(move |(offset, (before, after))| {
                    (before != after).then_some((base + offset as u32, *before, *after))
                })
        })
        .collect()
    }

    pub fn run(args: Vec<std::ffi::OsString>) -> Result<(), String> {
        let mut options = Options::parse_args(args)?;
        let trace_din_dispatch = std::env::var_os("L6_DIN_TRACE_DISPATCH").is_some();
        let trace_din_usb_forwarding = std::env::var_os("L6_DIN_USB_FORWARDING").is_some();
        let gdb_socket = options
            .logs
            .join(format!("din-dispatch-{}.gdb.sock", std::process::id()));
        if trace_din_dispatch {
            options.start_paused = true;
            options.gdb_socket = Some(gdb_socket.clone());
        }
        // USB startup is intentionally gated by a panel sequence on L6max.
        // Start disconnected, perform that sequence, then connect the host
        // bridge so its first enumeration attempt sees the running device.
        options.usb_enabled = false;
        options.midi_enabled = true;
        let mut guest = Headless::start(&options)?;
        let model = guest.engine.model();

        // Exploratory: capture the editor's own identity + global-setting
        // dump reply for either model, optionally after one parameter-change
        // request, to map the L6 dump layout field by field. Not a permanent
        // check; gated behind an env var so default runs are unaffected.
        if std::env::var_os("L6_EDITOR_DUMP_PROBE").is_some() {
            let inputs = guest.engine.inputs.as_ref().unwrap().clone();
            if model == l6fw::Model::L6max {
                guest.until(40_000).map_err(|e| e.to_string())?;
                inputs.tap(3, 50).map_err(|e| e.to_string())?;
                guest.until(42_000).map_err(|e| e.to_string())?;
                inputs.tap(3, 50).map_err(|e| e.to_string())?;
                guest.until(47_000).map_err(|e| e.to_string())?;
            } else {
                // L6 has no screen, but its USB editor interface is gated by
                // the same two recorder-function presses, verified at guest
                // ~40s/~42s with enumeration by ~47s (see the input diagnostic's
                // L6_MAIN_GPIO_EDITOR_SETUP sequence).
                guest.until(40_000).map_err(|e| e.to_string())?;
                inputs.tap(3, 50).map_err(|e| e.to_string())?;
                guest.until(42_000).map_err(|e| e.to_string())?;
                inputs.tap(3, 50).map_err(|e| e.to_string())?;
                guest.until(47_000).map_err(|e| e.to_string())?;
            }
            guest.engine.connect_usb()?;
            guest.wait(1500).map_err(|e| e.to_string())?;
            let names = port_names_for_model(model);
            let client =
                Client::new("din-editor-mixer-diagnostics").map_err(|e| format!("{e:?}"))?;
            let editor = open_port(&client, &endpoint_name(names[2]), "editor")?;
            guest.wait(300).map_err(|e| e.to_string())?;

            editor.send(&[0xF0, 0x7E, 0x00, 0x06, 0x01, 0xF7])?;
            let identity = editor
                .recv(Duration::from_secs(5))
                .ok_or("no identity reply from the editor session")?;
            println!("identity ({} bytes): {:02x?}", identity.len(), identity);

            editor.send(&[0xF0, 0x52, 0x00, 0x00, 0x2B, 0xF7])?;
            let dump1 = editor
                .recv(Duration::from_secs(5))
                .ok_or("no global setting dump reply")?;
            println!("dump1 ({} bytes): {:02x?}", dump1.len(), dump1);

            if let Ok(param) = std::env::var("L6_EDITOR_DUMP_PROBE_PARAM") {
                let mut parts = param.split(',');
                let p: u8 = parts
                    .next()
                    .ok_or("L6_EDITOR_DUMP_PROBE_PARAM needs param,value")?
                    .parse()
                    .map_err(|_| "bad param byte")?;
                let v: u8 = parts
                    .next()
                    .ok_or("L6_EDITOR_DUMP_PROBE_PARAM needs param,value")?
                    .parse()
                    .map_err(|_| "bad value byte")?;
                editor.send(&[0xF0, 0x52, 0x00, 0x00, 0x31, p, v, 0xF7])?;
                let ack = editor.recv(Duration::from_secs(5));
                println!("parameter-change {p:#04x}={v:#04x} ack: {ack:02x?}");
                guest.wait(300).map_err(|e| e.to_string())?;

                editor.send(&[0xF0, 0x52, 0x00, 0x00, 0x2B, 0xF7])?;
                let dump2 = editor
                    .recv(Duration::from_secs(5))
                    .ok_or("no second global setting dump reply")?;
                println!("dump2 ({} bytes): {:02x?}", dump2.len(), dump2);

                let diffs: Vec<(usize, u8, u8)> = dump1
                    .iter()
                    .zip(dump2.iter())
                    .enumerate()
                    .filter_map(|(i, (a, b))| (a != b).then_some((i, *a, *b)))
                    .collect();
                println!("diffs (index, before, after): {diffs:02x?}");
            }
            return Ok(());
        }

        if (trace_din_dispatch || trace_din_usb_forwarding) && model != l6fw::Model::L6 {
            return Err("L6 DIN diagnostics require L6 firmware".into());
        }
        let (
            dispatch_monitor,
            dispatch_running,
            handler_hits,
            state_dispatch_hits,
            queue_receiver_hits,
            event_queue_reader_hits,
            event_fallback_hits,
            main_task_entry_hits,
            main_init_hits,
            candidate_setter_hits,
            state_dispatch_args,
        ) = if trace_din_dispatch {
            let mut debugger = Gdb::connect(&gdb_socket).map_err(|e| e.to_string())?;
            debugger
                .breakpoint(0x8002f490, true)
                .map_err(|e| e.to_string())?;
            debugger
                .breakpoint(0x80020ca0, true)
                .map_err(|e| e.to_string())?;
            debugger
                .breakpoint(0x80020360, true)
                .map_err(|e| e.to_string())?;
            debugger
                .breakpoint(0x80020438, true)
                .map_err(|e| e.to_string())?;
            debugger
                .breakpoint(0x80020548, true)
                .map_err(|e| e.to_string())?;
            debugger
                .breakpoint(0x80020690, true)
                .map_err(|e| e.to_string())?;
            debugger
                .breakpoint(0x800344e0, true)
                .map_err(|e| e.to_string())?;
            debugger
                .breakpoint(0x80034530, true)
                .map_err(|e| e.to_string())?;
            debugger
                .breakpoint(0x8006dd70, true)
                .map_err(|e| e.to_string())?;
            for address in L6_MAIN_INIT_ADDRESSES {
                debugger
                    .breakpoint(address, true)
                    .map_err(|e| e.to_string())?;
            }
            for address in [0x8003d790, 0x8003e0c8, 0x8003e408] {
                debugger
                    .breakpoint(address, true)
                    .map_err(|e| e.to_string())?;
            }
            let running = Arc::new(AtomicBool::new(true));
            let handler_hits = Arc::new(AtomicUsize::new(0));
            let state_dispatch_hits = Arc::new(AtomicUsize::new(0));
            let queue_receiver_hits = Arc::new(AtomicUsize::new(0));
            let event_queue_reader_hits = Arc::new(AtomicUsize::new(0));
            let event_fallback_hits = Arc::new(AtomicUsize::new(0));
            let main_task_entry_hits = Arc::new(AtomicUsize::new(0));
            let main_init_hits = Arc::new(
                L6_MAIN_INIT_ADDRESSES
                    .map(|_| AtomicUsize::new(0))
                    .map(Arc::new),
            );
            let candidate_setter_hits = Arc::new(AtomicUsize::new(0));
            let state_dispatch_args = Arc::new(Mutex::new(Vec::new()));
            let worker_running = running.clone();
            let worker_handler_hits = handler_hits.clone();
            let worker_state_dispatch_hits = state_dispatch_hits.clone();
            let worker_queue_receiver_hits = queue_receiver_hits.clone();
            let worker_event_queue_reader_hits = event_queue_reader_hits.clone();
            let worker_event_fallback_hits = event_fallback_hits.clone();
            let worker_main_task_entry_hits = main_task_entry_hits.clone();
            let worker_main_init_hits = main_init_hits.clone();
            let worker_candidate_setter_hits = candidate_setter_hits.clone();
            let worker_state_dispatch_args = state_dispatch_args.clone();
            let monitor = thread::spawn(move || {
                while worker_running.load(Ordering::Relaxed) {
                    let Ok(_) = debugger.continue_until_stop(std::time::Duration::from_secs(120))
                    else {
                        break;
                    };
                    if !worker_running.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(pc) = debugger.read_register(15) else {
                        break;
                    };
                    let address = pc & !1;
                    let counter = match address {
                        0x8002f490 => Some(&worker_handler_hits),
                        0x80020ca0 => Some(&worker_state_dispatch_hits),
                        0x80020360 | 0x80020438 | 0x80020548 => Some(&worker_queue_receiver_hits),
                        0x80020690 => Some(&worker_event_queue_reader_hits),
                        0x800344e0 | 0x80034530 => Some(&worker_event_fallback_hits),
                        0x8006dd70 => Some(&worker_main_task_entry_hits),
                        0x8003d790 | 0x8003e0c8 | 0x8003e408 => Some(&worker_candidate_setter_hits),
                        _ => None,
                    };
                    if let Some(index) = L6_MAIN_INIT_ADDRESSES
                        .iter()
                        .position(|candidate| *candidate == address)
                    {
                        worker_main_init_hits[index].fetch_add(1, Ordering::Relaxed);
                    }
                    let Some(counter) = counter else {
                        if L6_MAIN_INIT_ADDRESSES.contains(&address) {
                            if debugger.breakpoint(address, false).is_err()
                                || debugger.request("s").is_err()
                                || debugger.breakpoint(address, true).is_err()
                            {
                                break;
                            }
                        }
                        continue;
                    };
                    counter.fetch_add(1, Ordering::Relaxed);
                    if address == 0x80020ca0 {
                        let Ok(r0) = debugger.read_register(0) else {
                            break;
                        };
                        let Ok(r1) = debugger.read_register(1) else {
                            break;
                        };
                        let Ok(r2) = debugger.read_register(2) else {
                            break;
                        };
                        let Ok(r3) = debugger.read_register(3) else {
                            break;
                        };
                        worker_state_dispatch_args
                            .lock()
                            .unwrap()
                            .push((r0, r1, r2, r3));
                    }
                    if debugger.breakpoint(address, false).is_err()
                        || debugger.request("s").is_err()
                        || debugger.breakpoint(address, true).is_err()
                    {
                        break;
                    }
                }
            });
            (
                Some(monitor),
                Some(running),
                Some(handler_hits),
                Some(state_dispatch_hits),
                Some(queue_receiver_hits),
                Some(event_queue_reader_hits),
                Some(event_fallback_hits),
                Some(main_task_entry_hits),
                Some(main_init_hits),
                Some(candidate_setter_hits),
                Some(state_dispatch_args),
            )
        } else {
            (
                None, None, None, None, None, None, None, None, None, None, None,
            )
        };
        let inputs = guest.engine.inputs.as_ref().unwrap().clone();
        if model == l6fw::Model::L6max {
            // L6max needs the recorder-screen sequence before its USB MIDI
            // interface is available. L6 has no screen or matching buttons;
            // its DIN UART and MIDI tasks initialize during normal boot.
            guest.recorder().map_err(|e| e.to_string())?;
            guest.until(40000).map_err(|e| e.to_string())?;
            inputs.tap(3, 50).map_err(|e| e.to_string())?;
            guest.until(42000).map_err(|e| e.to_string())?;
            inputs.tap(3, 50).map_err(|e| e.to_string())?;
            guest.until(47000).map_err(|e| e.to_string())?;
        } else {
            guest.until(2000).map_err(|e| e.to_string())?;
            if trace_din_usb_forwarding {
                guest.until(5000).map_err(|e| e.to_string())?;
            }
        }
        let names = port_names_for_model(model);
        if model == l6fw::Model::L6max {
            guest.engine.connect_usb()?;
            guest.wait(1000).map_err(|e| e.to_string())?;
        }

        let client = Client::new("din-editor-mixer-diagnostics").map_err(|e| format!("{e:?}"))?;
        if model == l6fw::Model::L6 {
            let din_name = din_port_name_for_model(model);
            let din = open_port(&client, &endpoint_name(din_name), "din")?;
            guest.wait(300).map_err(|e| e.to_string())?;
            if trace_din_usb_forwarding {
                let usb = guest.engine.usb.as_ref().unwrap();
                let midi = usb
                    .enumerate()
                    .map_err(|error| format!("cannot enumerate L6 USB MIDI: {error}"))?
                    .midi
                    .ok_or("L6 USB device has no MIDI endpoints")?;
                let _ = usb.try_receive(midi.input, 16384);
                let probes: [&[u8]; 4] = [
                    &[0xb3, 0x10, 0x20],
                    &[0x93, 0x45, 0x64],
                    &[0xf8],
                    &[0xf0, 0x7d, 0x01, 0xf7],
                ];
                for probe in probes {
                    din.send(probe)?;
                    let deadline = std::time::Instant::now() + Duration::from_secs(3);
                    let mut forwarded = false;
                    let mut forwarded_bytes = Vec::new();
                    while std::time::Instant::now() < deadline {
                        match usb.try_receive(midi.input, 16384) {
                            Ok(bytes) => {
                                let messages = l6max_host::usb_midi::decode(&bytes)
                                    .map_err(|error| format!("cannot decode USB MIDI: {error}"))?;
                                for (cable, data) in messages {
                                    if cable != 0
                                        || (forwarded_bytes.is_empty()
                                            && data.first() != probe.first())
                                    {
                                        continue;
                                    }
                                    forwarded_bytes.extend(data);
                                    if forwarded_bytes == probe {
                                        forwarded = true;
                                        break;
                                    }
                                    if !probe.starts_with(&forwarded_bytes) {
                                        forwarded_bytes.clear();
                                    }
                                }
                                if forwarded {
                                    break;
                                }
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(error) => {
                                return Err(format!("USB MIDI receive failed: {error}"));
                            }
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    if !forwarded {
                        return Err(format!(
                            "L6 DIN-to-USB forwarding mismatch: no USB cable-0 message matched {probe:02x?}"
                        ));
                    }
                    println!(
                        "L6 stock DIN-IN forwarded {probe:02x?} unchanged to the USB MIDI I/O port"
                    );
                }
                return Ok(());
            }
            if trace_din_dispatch {
                let mut previous_queue_items = l6_event_queue_items(&guest)?;
                let mut previous_reader_hits = event_queue_reader_hits
                    .as_ref()
                    .map(|hits| hits.load(Ordering::Relaxed))
                    .unwrap_or(0);
                let mut previous_fallback_hits = event_fallback_hits
                    .as_ref()
                    .map(|hits| hits.load(Ordering::Relaxed))
                    .unwrap_or(0);
                let mut previous_mixer_state = l6_mixer_snapshot(&guest)?;
                println!(
                    "L6 waiting event queue items before panel encoders: {previous_queue_items}"
                );
                let panel_args_start = state_dispatch_args
                    .as_ref()
                    .map(|args| args.lock().unwrap().len())
                    .unwrap_or(0);
                for encoder in 0..6 {
                    let before = candidate_setter_hits
                        .as_ref()
                        .map(|hits| hits.load(Ordering::Relaxed))
                        .unwrap_or(0);
                    inputs
                        .encoder(encoder, 1)
                        .map_err(|error| error.to_string())?;
                    guest.wait(400).map_err(|error| error.to_string())?;
                    let queue_items = l6_event_queue_items(&guest)?;
                    let reader_hits = event_queue_reader_hits
                        .as_ref()
                        .map(|hits| hits.load(Ordering::Relaxed))
                        .unwrap_or(0);
                    let fallback_hits = event_fallback_hits
                        .as_ref()
                        .map(|hits| hits.load(Ordering::Relaxed))
                        .unwrap_or(0);
                    let mixer_state = l6_mixer_snapshot(&guest)?;
                    let mixer_changes = l6_mixer_changes(&previous_mixer_state, &mixer_state);
                    let after = candidate_setter_hits
                        .as_ref()
                        .map(|hits| hits.load(Ordering::Relaxed))
                        .unwrap_or(0);
                    println!(
                        "L6 encoder {encoder}: waiting queue items {previous_queue_items} -> {queue_items}; Main queue-reader calls +{}; out-of-range routines 0x800344e0/0x80034530 +{}; mixer setters +{}; mixer RAM changes {mixer_changes:?}",
                        reader_hits.saturating_sub(previous_reader_hits),
                        fallback_hits.saturating_sub(previous_fallback_hits),
                        after.saturating_sub(before),
                    );
                    previous_queue_items = queue_items;
                    previous_reader_hits = reader_hits;
                    previous_fallback_hits = fallback_hits;
                    previous_mixer_state = mixer_state;
                }
                if let Some(args) = state_dispatch_args.as_ref() {
                    let args = args.lock().unwrap();
                    let encoder_events = args[panel_args_start..]
                        .iter()
                        .filter_map(|(r0, r1, parameter, delta)| {
                            (*r0 == 0 && *r1 == 5).then_some((*parameter, *delta))
                        })
                        .collect::<Vec<_>>();
                    println!(
                        "L6 panel encoder event parameters and deltas (r0=0, r1=5): {encoder_events:?}"
                    );
                }
            }
            let event_queue_count_before = guest.engine.management[0]
                .read_memory(0, 0x806b2f20, 4)
                .map_err(|error| format!("cannot read L6 event queue count: {error}"))?;
            let event_queue_count_before = u32::from_le_bytes(
                event_queue_count_before
                    .as_slice()
                    .try_into()
                    .map_err(|_| "invalid L6 event queue count read".to_owned())?,
            );
            let mixer_state_before = if trace_din_dispatch {
                Some(l6_mixer_snapshot(&guest)?)
            } else {
                None
            };
            let mut ring = indicators::rings(inputs.panel.snapshot().indicators)[0];
            let before = ring;
            let handler_hits_before = handler_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            let state_dispatch_hits_before = state_dispatch_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            let queue_receiver_hits_before = queue_receiver_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            let event_queue_reader_hits_before = event_queue_reader_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            let main_task_entry_hits_before = main_task_entry_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            let candidate_setter_hits_before = candidate_setter_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            let mut changes = Vec::new();
            for cc in 0..=127u8 {
                din.send(&[0xB0, cc, 0x7f])?;
                guest.wait(50).map_err(|e| e.to_string())?;
                let after = indicators::rings(inputs.panel.snapshot().indicators)[0];
                if after != ring {
                    changes.push((cc, ring, after));
                    ring = after;
                }
            }
            println!(
                "L6 default-setting DIN CC sweep: channel-1 ring {before:#x} -> {ring:#x}; changes {changes:?}"
            );
            if let (Some(monitor), Some(running)) = (dispatch_monitor, dispatch_running) {
                running.store(false, Ordering::Relaxed);
                guest.engine.management[0]
                    .stop()
                    .map_err(|error| format!("cannot stop guest after GDB trace: {error}"))?;
                monitor
                    .join()
                    .map_err(|_| "DIN dispatch trace thread panicked".to_owned())?;
            }
            let handler_hits_after = handler_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            let state_dispatch_hits_after = state_dispatch_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            let queue_receiver_hits_after = queue_receiver_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            let event_queue_reader_hits_after = event_queue_reader_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            let main_task_entry_hits_after = main_task_entry_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            let candidate_setter_hits_after = candidate_setter_hits
                .as_ref()
                .map(|hits| hits.load(Ordering::Relaxed))
                .unwrap_or(0);
            if trace_din_dispatch {
                let mixer_state_after = l6_mixer_snapshot(&guest)?;
                let mixer_state_before = mixer_state_before
                    .as_ref()
                    .ok_or("L6 mixer state snapshot was not captured")?;
                let changed_fields = l6_mixer_changes(mixer_state_before, &mixer_state_after);
                println!("L6 mixer state bytes changed during DIN sweep: {changed_fields:?}");
                let midi_handler_delta = handler_hits_after.saturating_sub(handler_hits_before);
                let event_submitter_delta =
                    state_dispatch_hits_after.saturating_sub(state_dispatch_hits_before);
                let event_reader_delta =
                    event_queue_reader_hits_after.saturating_sub(event_queue_reader_hits_before);
                let candidate_setter_delta =
                    candidate_setter_hits_after.saturating_sub(candidate_setter_hits_before);
                if midi_handler_delta != 128
                    || event_submitter_delta != 66
                    || event_reader_delta != event_submitter_delta
                    || candidate_setter_delta == 0
                    || changed_fields.is_empty()
                {
                    return Err(format!(
                        "L6 DIN dispatch mismatch: handler {midi_handler_delta}/128, submitted {event_submitter_delta}/66, Main consumed {event_reader_delta}, setters {candidate_setter_delta}, mixer-state changes {}",
                        changed_fields.len()
                    ));
                }
                let event_queue_count_after = guest.engine.management[0]
                    .read_memory(0, 0x806b2f20, 4)
                    .map_err(|error| format!("cannot read L6 event queue count: {error}"))?;
                let event_queue_count_after = u32::from_le_bytes(
                    event_queue_count_after
                        .as_slice()
                        .try_into()
                        .map_err(|_| "invalid L6 event queue count read".to_owned())?,
                );
                let event_queue_handle =
                    guest.engine.management[0]
                        .read_memory(0, 0x801f8f28, 4)
                        .map_err(|error| format!("cannot read L6 event queue handle: {error}"))?;
                let event_queue_handle = u32::from_le_bytes(
                    event_queue_handle
                        .as_slice()
                        .try_into()
                        .map_err(|_| "invalid L6 event queue handle read".to_owned())?,
                );
                if !(0x8000_0000..0x8100_0000).contains(&event_queue_handle) {
                    return Err(format!(
                        "unexpected L6 event queue handle {event_queue_handle:#x}"
                    ));
                }
                let event_queue = guest.engine.management[0]
                    .read_memory(0, event_queue_handle, 0x50)
                    .map_err(|error| format!("cannot inspect L6 event queue: {error}"))?;
                let queue_word = |offset: usize| {
                    u32::from_le_bytes(event_queue[offset..offset + 4].try_into().unwrap())
                };
                println!(
                    "L6 DIN calls during sweep: MIDI handler 0x8002f490 +{}, event submitter 0x80020ca0 +{}",
                    handler_hits_after.saturating_sub(handler_hits_before),
                    state_dispatch_hits_after.saturating_sub(state_dispatch_hits_before)
                );
                println!(
                    "L6 queue receiver calls during sweep: 0x80020360 / 0x80020438 / 0x80020548 +{}",
                    queue_receiver_hits_after.saturating_sub(queue_receiver_hits_before)
                );
                println!(
                    "L6 main-task event queue reader calls during sweep: 0x80020690 +{}",
                    event_queue_reader_hits_after.saturating_sub(event_queue_reader_hits_before)
                );
                println!(
                    "L6 Main task entry calls: 0x8006dd70 +{} during sweep ({} total before, {} total after)",
                    main_task_entry_hits_after.saturating_sub(main_task_entry_hits_before),
                    main_task_entry_hits_before,
                    main_task_entry_hits_after
                );
                if let Some(hits) = main_init_hits.as_ref() {
                    let calls = L6_MAIN_INIT_ADDRESSES
                        .iter()
                        .zip(hits.iter())
                        .map(|(address, count)| {
                            format!("{address:#x}={}", count.load(Ordering::Relaxed))
                        })
                        .collect::<Vec<_>>();
                    println!("L6 Main startup calls: {calls:?}");
                }
                println!(
                    "L6 candidate mixer-setter entries during sweep: 0x8003d790 / 0x8003e0c8 / 0x8003e408 +{}",
                    candidate_setter_hits_after.saturating_sub(candidate_setter_hits_before)
                );
                println!(
                    "L6 event queue item count: {event_queue_count_before} before, {event_queue_count_after} after the sweep"
                );
                println!(
                    "L6 event queue {event_queue_handle:#x}: waiting-to-send {}, waiting-to-receive {}, items {}/{}, item size {}",
                    queue_word(0x10),
                    queue_word(0x24),
                    queue_word(0x38),
                    queue_word(0x3c),
                    queue_word(0x40)
                );
                if let Some(args) = state_dispatch_args.as_ref() {
                    let args = args.lock().unwrap();
                    let mut parameters = args
                        .iter()
                        .filter_map(|(r0, r1, parameter, _)| {
                            (*r0 == 2 && *r1 == 3).then_some(*parameter)
                        })
                        .collect::<Vec<_>>();
                    parameters.sort_unstable();
                    parameters.dedup();
                    println!("L6 mapped-CC event parameters (r0=2, r1=3): {parameters:?}");
                }
            }
            let trace = std::fs::read_to_string(options.logs.join("engine.trace"))
                .map_err(|error| format!("cannot read DIN UART trace: {error}"))?;
            let accepted_bytes = trace
                .lines()
                .filter(|line| {
                    line.contains("LPUART2 (MIDI jacks) host RX ") && line.contains(" RE 1 RIE 1")
                })
                .filter_map(|line| {
                    let count = line.split("host RX ").nth(1)?.split(" bytes CTRL").next()?;
                    count.parse::<usize>().ok()
                })
                .sum::<usize>();
            let expected_bytes = 128 * 3;
            if accepted_bytes != expected_bytes {
                return Err(format!(
                    "L6 DIN receive mismatch: expected {expected_bytes} bytes at LPUART2 with RE/RIE enabled, observed {accepted_bytes}"
                ));
            }
            println!(
                "L6 DIN UART received all {accepted_bytes} bytes with RE/RIE enabled; channel-1 ring {before:#x} -> {ring:#x}; changes {changes:?}."
            );
            if changes.is_empty() {
                return Err("L6 stock DIN CC sweep did not change the mixer indicator ring".into());
            }
            let din_ring = guest.engine.management[0]
                .read_memory(0, 0x8062e5a8, expected_bytes)
                .map_err(|error| format!("cannot read L6 DIN byte ring: {error}"))?;
            let din_ring_state = guest.engine.management[0]
                .read_memory(0, 0x8062eda8, 0x20)
                .map_err(|error| format!("cannot read L6 DIN ring state: {error}"))?;
            let dispatch_map = guest.engine.management[0]
                .read_memory(0, 0x80629bb8, 128)
                .map_err(|error| format!("cannot read L6 MIDI dispatch map: {error}"))?;
            let expected_ring = (0..=127u8)
                .flat_map(|cc| [0xb0, cc, 0x7f])
                .collect::<Vec<_>>();
            let matching_ring_bytes = din_ring
                .iter()
                .zip(&expected_ring)
                .filter(|(actual, expected)| actual == expected)
                .count();
            let write_index = u32::from_le_bytes([
                din_ring_state[4],
                din_ring_state[5],
                din_ring_state[6],
                din_ring_state[7],
            ]);
            let read_index = u32::from_le_bytes([
                din_ring_state[12],
                din_ring_state[13],
                din_ring_state[14],
                din_ring_state[15],
            ]);
            if matching_ring_bytes != expected_bytes
                || write_index != expected_bytes as u32
                || read_index != write_index
            {
                return Err(format!(
                    "L6 raw DIN ring mismatch: bytes {matching_ring_bytes}/{expected_bytes}, write index {write_index}, read index {read_index}"
                ));
            }
            println!(
                "L6 raw DIN ring received and consumed all {matching_ring_bytes} injected bytes (write/read indices {write_index}/{read_index}); channel-1 mixer ring remains {ring:#x}"
            );
            let mapped_ccs = dispatch_map
                .iter()
                .enumerate()
                .filter_map(|(cc, mapping)| (*mapping != 0xff).then_some((cc, *mapping)))
                .collect::<Vec<_>>();
            println!("L6 runtime MIDI CC -> parameter-map indices: {mapped_ccs:?}");
            return Ok(());
        }
        let editor = open_port(&client, &endpoint_name(names[2]), "editor")?;
        let mixer = open_port(&client, &endpoint_name(names[1]), "mixer")?;
        let din = open_port(
            &client,
            &endpoint_name(din_port_name_for_model(model)),
            "din",
        )?;
        guest.wait(300).map_err(|e| e.to_string())?;

        // The editor's own session: identity, then a global setting dump
        // request, both required before the firmware accepts a parameter
        // change (see docs/firmware/protocols/editor-midi.md).
        editor.send(&[0xF0, 0x7E, 0x00, 0x06, 0x01, 0xF7])?;
        editor
            .recv(Duration::from_secs(3))
            .ok_or("no identity reply from the editor session")?;
        editor.send(&[0xF0, 0x52, 0x00, 0x00, 0x2B, 0xF7])?;
        let dump = editor
            .recv(Duration::from_secs(3))
            .ok_or("no global setting dump reply")?;
        if dump.len() < 182 {
            return Err(format!(
                "global setting dump too short: {} bytes",
                dump.len()
            ));
        }

        // parameterChange 03 = mixerControlViaMidi, value 1.
        editor.send(&[0xF0, 0x52, 0x00, 0x00, 0x31, 0x03, 0x01, 0xF7])?;
        editor
            .recv(Duration::from_secs(3))
            .ok_or("no acknowledgement for enabling Mixer Control via MIDI")?;
        guest.wait(300).map_err(|e| e.to_string())?;

        let ring = || indicators::rings(inputs.panel.snapshot().indicators)[0];

        // Control: the known-working path must still move the ring, or this
        // check's display sampling and CC framing are themselves broken.
        let before_usb = ring();
        mixer.send(&[0xB0, 0x51, 80])?;
        guest.wait(500).map_err(|e| e.to_string())?;
        let after_usb = ring();
        if before_usb == after_usb {
            return Err("control check failed: a mixer CC on the USB Mixer Control Port did not move the ring".into());
        }
        println!(
            "control check passed: USB Mixer Control Port CC moved the ring ({before_usb:#x} -> {after_usb:#x})"
        );

        // The actual question. Stock firmware never applies this; firmware
        // built with the din-mixer-control patch should.
        let expect_fixed = std::env::var_os("L6_DIN_MIXER_CONTROL_FIXED").is_some();
        let before_din = ring();
        din.send(&[0xB0, 0x51, 40])?;
        guest.wait(500).map_err(|e| e.to_string())?;
        let after_din = ring();
        let moved = before_din != after_din;
        if moved != expect_fixed {
            return Err(format!(
                "DIN MIDI IN {} the mixer with Mixer Control via MIDI enabled ({before_din:#x} \
                 -> {after_din:#x}); expected it to {} given L6_DIN_MIXER_CONTROL_FIXED={}",
                if moved {
                    "now reaches"
                } else {
                    "still does not reach"
                },
                if expect_fixed {
                    "reach it"
                } else {
                    "not reach it"
                },
                expect_fixed as u8,
            ));
        }
        println!(
            "confirmed: a fader CC sent into the DIN MIDI IN jack {} the mixer, with Mixer Control via MIDI enabled ({})",
            if moved { "moves" } else { "does not move" },
            if expect_fixed {
                "din-mixer-control patch"
            } else {
                "stock firmware"
            },
        );

        // Control: the setting really took effect, independent of the IN
        // side above. The DIN OUT echo added for the LPUART3 TX fix is
        // gated on mixerControlViaMidi == 0; with it on, a panel mute press
        // must not produce that echo.
        let _ = din.drain();
        inputs.tap(22, 50).map_err(|e| e.to_string())?;
        guest.wait(500).map_err(|e| e.to_string())?;
        let echoed = din.drain();
        if !echoed.is_empty() {
            return Err(format!(
                "Mixer Control via MIDI did not take effect: the DIN OUT echo still fired \
                 after a mute press ({echoed:02x?})"
            ));
        }
        println!(
            "confirmed: Mixer Control via MIDI is in effect (the DIN OUT mute echo is suppressed, as it should be once it is on)"
        );

        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn run(args: Vec<std::ffi::OsString>) -> Result<(), String> {
    imp::run(args)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn run(_args: Vec<std::ffi::OsString>) -> Result<(), String> {
    Err("MIDI diagnostics currently require macOS (CoreMIDI)".into())
}
