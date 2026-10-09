//! ALSA sequencer client subscribed to the emulator's mixer-control port.
use super::{MidiClient, mixer_endpoint};
use alsa::seq::{
    Addr, ClientIter, EvCtrl, Event, EventType, PortCap, PortIter, PortSubscribe, PortType, Seq,
};
use std::ffi::CString;

struct AlsaClient {
    seq: Seq,
    mine: Addr,
    target: Addr,
}
fn e(error: alsa::Error) -> String {
    format!("ALSA {error}")
}
fn find_port(seq: &Seq) -> Option<Addr> {
    let name = mixer_endpoint();
    ClientIter::new(seq).find_map(|client| {
        PortIter::new(seq, client.get_client())
            .find_map(|port| (port.get_name().ok() == Some(name.as_str())).then(|| port.addr()))
    })
}
pub fn connect_os() -> Result<Box<dyn MidiClient>, String> {
    let seq = Seq::open(None, None, true).map_err(e)?;
    seq.set_client_name(&CString::new("L6max MIDI integration check").unwrap())
        .map_err(e)?;
    let port = seq
        .create_simple_port(
            &CString::new("integration").unwrap(),
            PortCap::READ | PortCap::WRITE | PortCap::SUBS_READ | PortCap::SUBS_WRITE,
            PortType::MIDI_GENERIC | PortType::APPLICATION,
        )
        .map_err(e)?;
    let mine = Addr {
        client: seq.client_id().map_err(e)?,
        port,
    };
    let target = find_port(&seq).ok_or("missing native MIDI port")?;
    for (sender, dest) in [(target, mine), (mine, target)] {
        let subscription = PortSubscribe::empty().map_err(e)?;
        subscription.set_sender(sender);
        subscription.set_dest(dest);
        seq.subscribe_port(&subscription).map_err(e)?;
    }
    Ok(Box::new(AlsaClient { seq, mine, target }))
}
impl MidiClient for AlsaClient {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        if let [status, param, value] = *bytes {
            let mut event = Event::new(
                EventType::Controller,
                &EvCtrl {
                    channel: status & 0x0f,
                    param: u32::from(param),
                    value: i32::from(value),
                },
            );
            event.set_source(self.mine.port);
            event.set_dest(self.target);
            event.set_direct();
            self.seq.event_output_direct(&mut event).map_err(e)?;
            return Ok(());
        }
        Err("ALSA client sends 3-byte Control Change only".into())
    }
    fn poll(&mut self) -> Result<Option<[u8; 3]>, String> {
        let mut input = self.seq.input();
        while let Ok(event) = input.event_input() {
            if let Some(ctrl) = event.get_data::<EvCtrl>() {
                return Ok(Some([
                    0xb0 | ctrl.channel,
                    ctrl.param as u8,
                    ctrl.value as u8,
                ]));
            }
        }
        Ok(None)
    }
    fn endpoint_present(&mut self) -> Option<bool> {
        Some(find_port(&self.seq).is_some())
    }
}
