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
//! Current, confirmed result: the DIN jack does not reach the mixer in
//! either direction, even with the setting enabled through this exact
//! session. See the "Physical MIDI IN/OUT (DIN/TRS) jacks" section of
//! `docs/emulator/usb.md`.

#[cfg(target_os = "macos")]
mod imp {
    use coremidi::{
        Client, Destination, Destinations, InputPort, OutputPort, PacketBuffer, Source,
    };
    use l6max_host::{
        din_midi, engine::Options, headless::Headless, host_midi::PORT_NAMES,
        host_midi::endpoint_name, indicators,
    };
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    struct Port {
        output: OutputPort,
        destination: Destination,
        #[allow(dead_code)]
        input: InputPort,
        #[allow(dead_code)]
        source: Source,
        rx: Receiver<Vec<u8>>,
    }
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

    pub fn run() -> Result<(), String> {
        let mut options = Options::parse()?;
        options.usb_enabled = true;
        options.midi_enabled = true;
        let guest = Headless::start(&options)?;
        guest.recorder().map_err(|e| e.to_string())?;
        guest.until(47000).map_err(|e| e.to_string())?;
        let inputs = guest.engine.inputs.as_ref().unwrap().clone();

        let client = Client::new("din-editor-mixer-smoke").map_err(|e| format!("{e:?}"))?;
        let editor = open_port(&client, &endpoint_name(PORT_NAMES[2]), "editor")?;
        let mixer = open_port(&client, &endpoint_name(PORT_NAMES[1]), "mixer")?;
        let din = open_port(&client, &endpoint_name(din_midi::PORT_NAME), "din")?;
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

        // The actual question.
        let before_din = ring();
        din.send(&[0xB0, 0x51, 40])?;
        guest.wait(500).map_err(|e| e.to_string())?;
        let after_din = ring();
        if before_din != after_din {
            return Err(format!(
                "DIN MIDI IN now reaches the mixer with Mixer Control via MIDI enabled \
                 ({before_din:#x} -> {after_din:#x}); update docs/emulator/usb.md, this test, \
                 and the DIN MIDI protocol notes to match"
            ));
        }
        println!(
            "confirmed: a fader CC sent into the DIN MIDI IN jack does not move the mixer, even with Mixer Control via MIDI enabled"
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
fn main() {
    if let Err(e) = imp::run() {
        eprintln!("din-editor-mixer-smoke: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("din-editor-mixer-smoke: macOS only (uses CoreMIDI directly)");
    std::process::exit(1);
}
