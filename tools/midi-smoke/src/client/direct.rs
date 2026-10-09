//! USB MIDI straight to the emulated device, bypassing the host MIDI system.
use super::{MIXER_CABLE, MidiClient};
use l6max_host::{
    usb::{Endpoints, UsbHost},
    usb_midi::{Encoder, decode},
};
use std::{collections::VecDeque, sync::Arc};

pub struct DirectClient {
    usb: Arc<UsbHost>,
    pair: Endpoints,
    encoder: Encoder,
    pending: VecDeque<[u8; 3]>,
}
impl DirectClient {
    pub fn connect(usb: Arc<UsbHost>) -> Result<Self, String> {
        let pair = usb
            .enumerate()
            .map_err(|e| e.to_string())?
            .midi
            .ok_or("no MIDI interface")?;
        Ok(Self {
            usb,
            pair,
            encoder: Encoder::default(),
            pending: VecDeque::new(),
        })
    }
}
impl MidiClient for DirectClient {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let wire = self.encoder.push(MIXER_CABLE as u8, bytes);
        self.usb
            .send(self.pair.output, &wire)
            .map_err(|e| e.to_string())
    }
    fn poll(&mut self) -> Result<Option<[u8; 3]>, String> {
        if self.pending.is_empty() {
            match self.usb.try_receive(self.pair.input, 16384) {
                Ok(bytes) => {
                    for (cable, data) in decode(&bytes).map_err(|e| e.to_string())? {
                        if usize::from(cable) == MIXER_CABLE && data.len() == 3 {
                            if data[0] & 0xf0 == 0xb0 {
                                self.pending.push_back([data[0], data[1], data[2]]);
                            }
                        }
                    }
                }
                Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(self.pending.pop_front())
    }
}
