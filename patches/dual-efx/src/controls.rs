//! Pure control-surface gesture logic. Firmware event delivery and timing are
//! revision-specific and must be connected separately.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EfxSelectAction {
    None,
    SelectNext,
    ToggleInspector,
}

/// Recognizes the observed L6max panel EFX SEL frames in a byte stream.
/// The caller must reset this on transport loss or protocol reinitialization.
#[derive(Default)]
pub struct PanelEfxDecoder {
    phase: u8,
    pressed: bool,
    column: u8,
}

impl PanelEfxDecoder {
    pub fn feed(&mut self, byte: u8) -> Option<bool> {
        if byte & 0x80 != 0 {
            self.phase = if byte == 0x90 || byte == 0x91 { 1 } else { 0 };
            self.pressed = byte == 0x91;
            return None;
        }
        match self.phase {
            1 => { self.column = byte; self.phase = 2; None }
            2 => { self.phase = 0; (self.column == 4 && byte == 6).then_some(self.pressed) }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Default)]
enum PressState {
    #[default]
    Idle,
    Down(u32),
    Ambiguous,
}

#[derive(Default)]
pub struct EfxSelectGesture {
    press: PressState,
    last_sequence: Option<u32>,
}

impl EfxSelectGesture {
    /// Defers the stock selection until release, so a hold can open the
    /// inspector without also advancing the effect. The caller supplies a
    /// threshold in the same wrapping tick units as `now`.
    pub fn edge(&mut self, pressed: bool, now: u32, hold_ticks: u32) -> EfxSelectAction {
        if self.last_sequence.take().is_some() {
            self.press = PressState::Idle;
        }
        self.classify_edge(pressed, now, hold_ticks)
    }

    /// Classifies an edge carrying a nonzero sequence assigned at reception.
    /// A gap discards an unfinished gesture before considering the new edge.
    /// Sequence zero means the event could not be associated with reception.
    pub fn sequenced_edge(&mut self, pressed: bool, now: u32, sequence: u32, hold_ticks: u32) -> EfxSelectAction {
        if sequence == 0 {
            self.cancel();
            return EfxSelectAction::None;
        }
        if let Some(previous) = self.last_sequence {
            let next = previous.wrapping_add(1);
            let expected = if next == 0 { 1 } else { next };
            if sequence != expected {
                self.press = PressState::Idle;
            }
        } else {
            self.press = PressState::Idle;
        }
        self.last_sequence = Some(sequence);
        self.classify_edge(pressed, now, hold_ticks)
    }

    fn classify_edge(&mut self, pressed: bool, now: u32, hold_ticks: u32) -> EfxSelectAction {
        if pressed {
            // A repeated press may mean a release was lost before sequence
            // assignment. Discard the ambiguous gesture until a release.
            self.press = match self.press {
                PressState::Idle => PressState::Down(now),
                PressState::Down(_) | PressState::Ambiguous => PressState::Ambiguous,
            };
            return EfxSelectAction::None;
        }
        match core::mem::take(&mut self.press) {
            PressState::Idle | PressState::Ambiguous => EfxSelectAction::None,
            PressState::Down(start) if now.wrapping_sub(start) >= hold_ticks => EfxSelectAction::ToggleInspector,
            PressState::Down(_) => EfxSelectAction::SelectNext,
        }
    }

    /// Discards a pending press when its window or control context is lost.
    pub fn cancel(&mut self) {
        self.press = PressState::Idle;
        self.last_sequence = None;
    }
}
