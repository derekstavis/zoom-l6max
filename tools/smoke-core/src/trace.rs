//! Incremental substring counters; retain only the longest pattern's boundary.
use l6fw::Model;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub(crate) struct TraceCursor {
    offset: u64,
    tail: Vec<u8>,
    pub counts: BTreeMap<String, u64>,
    matcher: Option<(aho_corasick::AhoCorasick, Vec<String>)>,
    uart_matcher: Option<(aho_corasick::AhoCorasick, Vec<String>)>,
    uart_tail: Vec<u8>,
    line: Vec<u8>,
}
impl Default for TraceCursor {
    fn default() -> Self {
        let mut cursor = Self {
            offset: 0,
            tail: vec![],
            counts: BTreeMap::new(),
            matcher: None,
            uart_matcher: None,
            uart_tail: vec![],
            line: vec![],
        };
        cursor.watch("l6 panel USART1 TX a1\n").unwrap();
        cursor
    }
}
impl TraceCursor {
    pub fn restart(&mut self) {
        self.offset = 0;
        self.tail.clear();
        self.uart_tail.clear();
        self.line.clear();
        self.counts.values_mut().for_each(|c| *c = 0);
    }
    pub fn watch(&mut self, pattern: &str) -> Result<(), String> {
        if pattern.is_empty() {
            return Err("trace patterns cannot be empty".into());
        }
        self.counts.entry(pattern.into()).or_default();
        self.matcher = None;
        self.uart_matcher = None;
        Ok(())
    }
    fn count(&mut self, bytes: &[u8], uart: bool) -> Result<(), String> {
        let (tail, matcher) = if uart {
            (&mut self.uart_tail, &mut self.uart_matcher)
        } else {
            (&mut self.tail, &mut self.matcher)
        };
        let mut combined = tail.clone();
        combined.extend_from_slice(bytes);
        if matcher.is_none() {
            let patterns: Vec<String> = self
                .counts
                .keys()
                .filter(|p| p.starts_with("l6 panel USART1 TX ") == uart)
                .cloned()
                .collect();
            let compiled = aho_corasick::AhoCorasick::new(&patterns).map_err(|e| e.to_string())?;
            *matcher = Some((compiled, patterns));
        }
        let (compiled, patterns) = matcher.as_ref().unwrap();
        for hit in compiled.find_overlapping_iter(&combined) {
            if hit.end() > tail.len() {
                *self
                    .counts
                    .get_mut(&patterns[hit.pattern().as_usize()])
                    .unwrap() += 1;
            }
        }
        let retain = patterns
            .iter()
            .map(|s| s.len().saturating_sub(1))
            .max()
            .unwrap_or(0);
        *tail = combined[combined.len().saturating_sub(retain)..].to_vec();
        Ok(())
    }
    fn consume(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.count(bytes, false)?;
        self.line.extend_from_slice(bytes);
        let mut uart = Vec::new();
        let mut consumed = 0;
        for (i, byte) in self.line.iter().enumerate() {
            if *byte == b'\n' {
                let line = &self.line[consumed..=i];
                if line.starts_with(b"l6 panel USART1 TX ") {
                    uart.extend_from_slice(line);
                }
                consumed = i + 1;
            }
        }
        self.line.drain(..consumed);
        if self.line.len() > 1024 * 1024 {
            return Err("trace line exceeds one MiB".into());
        }
        self.count(&uart, true)
    }
    pub fn sample(&mut self, path: &Path) -> Result<u64, String> {
        let mut file = match fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(self.counts["l6 panel USART1 TX a1\n"]);
            }
            Err(e) => return Err(e.to_string()),
        };
        let length = file.metadata().map_err(|e| e.to_string())?.len();
        if length < self.offset {
            return Err("trace truncated during run".into());
        }
        file.seek(SeekFrom::Start(self.offset))
            .map_err(|e| e.to_string())?;
        // Freeze the read length: a noisy producer cannot monopolize a tick.
        let mut file = file.take(length - self.offset);
        let mut buffer = [0; 65536];
        loop {
            let size = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if size == 0 {
                break;
            }
            self.offset += size as u64;
            self.consume(&buffer[..size])?;
        }
        Ok(self.counts["l6 panel USART1 TX a1\n"])
    }
}

/// L6max panel wire protocol, retained from the original GPIO control smoke.
/// MAIN GPIO controls have no panel UART packet and are rejected here.
pub(crate) fn button_packets(model: Model, control: &str) -> Result<[String; 2], String> {
    let id = crate::control_target(model, control)?;
    let (row, col) = if model == Model::L6 {
        match id {
            7..=26 => ((id - 7) / 3, (id - 7) % 3 + 1),
            28 => (7, 1),
            29 => (7, 2),
            2 => (1, 0),
            3 => (2, 0),
            6 => (3, 0),
            47 => (4, 0),
            _ => {
                return Err(format!("{control} has no recovered L6 panel UART report"));
            }
        }
    } else {
        match id {
            1 => (5, 5),
            2 => (4, 5),
            3 => (7, 5),
            6 => (6, 5),
            7..=46 => ((id - 7) / 5, (id - 7) % 5 + 1),
            47 => (0, 0),
            _ => {
                return Err(format!(
                    "{control} is a MAIN control, without a panel UART report"
                ));
            }
        }
    };
    Ok([true, false].map(|pressed| format!("l6 panel USART1 TX {:02x}\nl6 panel USART1 TX {col:02x}\nl6 panel USART1 TX {row:02x}\n", if pressed { 0x91 } else { 0x90 })))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunk_boundaries_and_overlaps_are_counted_once_with_bounded_memory() {
        let mut cursor = TraceCursor::default();
        cursor.watch("aba").unwrap();
        for bytes in [b"a".as_slice(), b"bab", b"aba"] {
            cursor.consume(bytes).unwrap();
        }
        assert_eq!(cursor.counts["aba"], 3);
        cursor.consume(&vec![b'x'; 100_000]).unwrap();
        assert_eq!(cursor.counts["aba"], 3);
        assert!(cursor.tail.len() < 32);
    }
    #[test]
    fn master_switch_report_polarity_matches_the_stock_panel_trace() {
        let [down, up] = button_packets(Model::L6max, "master-submix-switch").unwrap();
        assert_eq!(
            down,
            "l6 panel USART1 TX 91\nl6 panel USART1 TX 00\nl6 panel USART1 TX 00\n"
        );
        assert_eq!(
            up,
            "l6 panel USART1 TX 90\nl6 panel USART1 TX 00\nl6 panel USART1 TX 00\n"
        );
    }
    #[test]
    fn uart_packets_survive_interleaved_logs_and_fragmented_reads() {
        let mut cursor = TraceCursor::default();
        let packet = "l6 panel USART1 TX 91\nl6 panel USART1 TX 04\nl6 panel USART1 TX 03\n";
        cursor.watch(packet).unwrap();
        for chunk in [
            "l6 panel USART1 TX 91\nmain CPU activity\nl6 panel USART1 T",
            "X 04\nother trace\nl6 panel USART1 TX 03",
            "\n",
        ] {
            cursor.consume(chunk.as_bytes()).unwrap();
        }
        assert_eq!(cursor.counts[packet], 1);
        cursor.consume(b"unrelated\n").unwrap();
        assert_eq!(cursor.counts[packet], 1);
    }
    #[test]
    fn l6_phantom_and_mode_controls_use_recovered_panel_reports() {
        let [down, up] = button_packets(Model::L6, "phantom-48v").unwrap();
        assert_eq!(
            down,
            "l6 panel USART1 TX 91\nl6 panel USART1 TX 01\nl6 panel USART1 TX 00\n"
        );
        assert_eq!(
            up,
            "l6 panel USART1 TX 90\nl6 panel USART1 TX 01\nl6 panel USART1 TX 00\n"
        );
        let [mode, _] = button_packets(Model::L6, "mode.level").unwrap();
        assert_eq!(
            mode,
            "l6 panel USART1 TX 91\nl6 panel USART1 TX 01\nl6 panel USART1 TX 07\n"
        );
    }
}
