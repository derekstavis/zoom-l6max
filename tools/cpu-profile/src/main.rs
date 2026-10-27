//! Compare QEMU CPU use, guest clock progression, and active Cortex-M exceptions.
use l6max_host::{
    display::PublishedDisplay,
    engine::{Engine, Options},
    input::EngineInput,
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::Write,
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn cpu_seconds(pid: u32) -> Result<f64, String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "time="])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("ps could not read QEMU CPU time".into());
    }
    let time = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    time.trim().split(':').try_fold(0.0, |total, part| {
        part.parse::<f64>()
            .map(|part| total * 60.0 + part)
            .map_err(|e| e.to_string())
    })
}
fn pump(display: &PublishedDisplay, duration: Duration) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < duration {
        display.sample().map_err(|e| e.to_string())?;
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
fn guest_stamp(input: &EngineInput, display: &PublishedDisplay) -> Result<u32, String> {
    // A single encoder command supplies a guest-time acknowledgment without
    // pressing or holding a button. The benchmark consumes display updates.
    let applied = input.snapshot().applied;
    input.encoder(2, 1).map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        pump(display, Duration::from_millis(5))?;
        let status = input.snapshot();
        if status.applied > applied {
            return status
                .last_ack
                .map(|ack| ack.duration_ms)
                .ok_or("missing acknowledgment".into());
        }
        if start.elapsed() > Duration::from_secs(3) {
            return Err("guest clock probe timed out".into());
        }
    }
}
fn exception_number(registers: &str) -> Option<u16> {
    registers
        .split_whitespace()
        .find_map(|word| word.strip_prefix("XPSR="))
        .and_then(|word| u32::from_str_radix(word, 16).ok())
        .map(|xpsr| (xpsr & 0x1ff) as u16)
}

fn edma_tcd_summary(qmp: &l6max_host::qmp::QmpClient, channel: u32) -> Result<String, String> {
    let address = 0x400e_9000 + channel * 32;
    let bytes = qmp
        .read_memory(0, address, 32)
        .map_err(|error| error.to_string())?;
    let u32_at = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let u16_at = |offset: usize| u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
    Ok(format!(
        "channel={channel} SADDR={:#010x} NBYTES={:#x} DADDR={:#010x} CITER={:#06x}/{:#06x} CSR={:#06x} DLAST_SGA={:#010x}",
        u32_at(0),
        u32_at(8),
        u32_at(16),
        u16_at(22),
        u16_at(30),
        u16_at(28),
        u32_at(24),
    ))
}

#[derive(Debug)]
struct ProfileOptions {
    warmup: Duration,
    measure: Duration,
    sample_interval: Duration,
    guest_clock: bool,
    engine_args: Vec<OsString>,
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<ProfileOptions, String> {
    let mut parsed = ProfileOptions {
        warmup: Duration::from_secs(45),
        measure: Duration::from_secs(20),
        sample_interval: Duration::from_millis(157),
        guest_clock: true,
        engine_args: Vec::new(),
    };
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--warmup-seconds") | Some("--measure-seconds") | Some("--sample-interval-ms") => {
                let flag = argument.to_string_lossy().into_owned();
                let value = arguments
                    .next()
                    .ok_or_else(|| format!("{flag} requires a positive integer"))?;
                let seconds = value
                    .to_str()
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|seconds| *seconds > 0)
                    .ok_or_else(|| format!("{flag} requires a positive integer"))?;
                match flag.as_str() {
                    "--warmup-seconds" => parsed.warmup = Duration::from_secs(seconds),
                    "--measure-seconds" => parsed.measure = Duration::from_secs(seconds),
                    _ => parsed.sample_interval = Duration::from_millis(seconds),
                }
            }
            Some("--skip-guest-clock") => parsed.guest_clock = false,
            _ => parsed.engine_args.push(argument),
        }
    }
    Ok(parsed)
}

fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<(), String> {
    let profile = parse_args(arguments)?;
    let options = Options::parse_args(profile.engine_args)?;
    let engine = Engine::start(&options)?;
    let inputs = engine.inputs.as_ref().ok_or("QEMU is required")?;
    let display = PublishedDisplay::new(
        engine.display_memory.as_ref().unwrap().map(),
        inputs.main.clone(),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "Warming up for {:.1}s, then measuring for {:.1}s at {:.0}ms samples; timing mode {:?}",
        profile.warmup.as_secs_f64(),
        profile.measure.as_secs_f64(),
        profile.sample_interval.as_secs_f64() * 1000.0,
        options.timing
    );
    pump(&display, profile.warmup)?;
    let jit_before = engine.management[0]
        .jit_stats()
        .map_err(|error| format!("could not read TCG JIT stats before measurement: {error}"))?;
    let guest_before = profile
        .guest_clock
        .then(|| guest_stamp(&inputs.panel, &display))
        .transpose()?;
    let cpu_before = cpu_seconds(engine.process_id().unwrap())?;
    let start = Instant::now();
    let mut pcs = [BTreeMap::new(), BTreeMap::new()];
    let mut exceptions = [BTreeMap::new(), BTreeMap::new()];
    let mut pc_exceptions = [BTreeMap::new(), BTreeMap::new()];
    let mut indicator_samples = Vec::new();
    while start.elapsed() < profile.measure {
        let mut sampled_pcs = [String::new(), String::new()];
        let mut sampled_exceptions = [None, None];
        for (cpu, counts) in pcs.iter_mut().enumerate() {
            let regs = engine.management[0]
                .registers(cpu as i64)
                .map_err(|e| e.to_string())?;
            let pc = regs
                .split_whitespace()
                .find_map(|word| word.strip_prefix("R15="))
                .ok_or("PC missing")?;
            sampled_pcs[cpu] = pc.to_owned();
            *counts.entry(pc.to_owned()).or_insert(0u32) += 1;
            if let Some(exception) = exception_number(&regs) {
                sampled_exceptions[cpu] = Some(exception);
                *exceptions[cpu].entry(exception).or_insert(0u32) += 1;
                *pc_exceptions[cpu]
                    .entry((pc.to_owned(), exception))
                    .or_insert(0u32) += 1;
            }
        }
        let panel = inputs.panel.snapshot();
        let rings = l6max_host::indicators::rings(panel.indicators);
        indicator_samples.push((
            start.elapsed().as_millis(),
            panel.guest_ms,
            sampled_pcs,
            sampled_exceptions,
            panel.indicators,
            rings,
        ));
        pump(&display, profile.sample_interval)?;
    }
    let cpu_after = cpu_seconds(engine.process_id().unwrap())?;
    let jit_after = engine.management[0]
        .jit_stats()
        .map_err(|error| format!("could not read TCG JIT stats after measurement: {error}"))?;
    let guest_after = profile
        .guest_clock
        .then(|| guest_stamp(&inputs.panel, &display))
        .transpose()?;
    let wall = start.elapsed().as_secs_f64();
    let cpu = 100.0 * (cpu_after - cpu_before) / wall;
    if let (Some(guest_before), Some(guest_after)) = (guest_before, guest_after) {
        let guest = guest_after.wrapping_sub(guest_before) as f64 / 1000.0;
        println!(
            "QEMU CPU {cpu:.1}% (one core = 100%); wall {wall:.3}s; guest {guest:.3}s; ratio {:.3}",
            guest / wall
        );
        if !(0.95..=1.05).contains(&(guest / wall)) {
            return Err("guest clock differs from wall time by more than 5%".into());
        }
    } else {
        println!(
            "QEMU CPU {cpu:.1}% (one core = 100%); wall {wall:.3}s; guest-clock probe skipped"
        );
    }
    let indicator_path = options.logs.join("startup-indicators.tsv");
    let save_samples = || -> std::io::Result<()> {
        let mut indicator_file = std::io::BufWriter::new(std::fs::File::create(&indicator_path)?);
        write!(
            indicator_file,
            "wall_ms\tguest_ms\tmain_pc\tmain_exception\tpanel_pc\tpanel_exception"
        )?;
        for row in 0..8 {
            write!(indicator_file, "\trow{row}")?;
        }
        for ring in 0..8 {
            write!(indicator_file, "\tring{ring}")?;
        }
        writeln!(indicator_file)?;
        for (wall_ms, guest_ms, sampled_pcs, sampled_exceptions, rows, rings) in &indicator_samples
        {
            let main_exception =
                sampled_exceptions[0].map_or_else(String::new, |value| value.to_string());
            let panel_exception =
                sampled_exceptions[1].map_or_else(String::new, |value| value.to_string());
            write!(
                indicator_file,
                "{wall_ms}\t{guest_ms}\t{}\t{main_exception}\t{}\t{panel_exception}",
                sampled_pcs[0], sampled_pcs[1]
            )?;
            for row in rows {
                write!(indicator_file, "\t{row:06x}")?;
            }
            for ring in rings {
                write!(indicator_file, "\t{ring:03x}")?;
            }
            writeln!(indicator_file)?;
        }
        indicator_file.flush()
    };
    save_samples().map_err(|error| error.to_string())?;
    println!("Startup LED/CPU samples: {}", indicator_path.display());
    println!("TCG JIT stats before measurement:\n{jit_before}");
    println!("TCG JIT stats after measurement:\n{jit_after}");
    for (cpu, counts) in pcs.iter().enumerate() {
        let mut hottest: Vec<_> = counts.iter().collect();
        hottest.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        println!(
            "CPU {cpu} most sampled PCs: {:?}",
            &hottest[..hottest.len().min(12)]
        );
        println!(
            "CPU {cpu} sampled xPSR exception numbers: {:?}",
            exceptions[cpu]
        );
        let mut hot_exception_pcs: Vec<_> = pc_exceptions[cpu].iter().collect();
        hot_exception_pcs.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        println!(
            "CPU {cpu} most sampled PC/exception pairs: {:?}",
            &hot_exception_pcs[..hot_exception_pcs.len().min(12)]
        );
    }
    for channel in 12..=15 {
        match edma_tcd_summary(&engine.management[0], channel) {
            Ok(summary) => println!("eDMA TCD: {summary}"),
            Err(error) => println!("eDMA TCD channel {channel}: unavailable ({error})"),
        }
    }
    Ok(())
}
fn main() {
    if let Err(error) = run(std::env::args_os().skip(1)) {
        eprintln!("cpu-profile: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{Duration, parse_args};
    use std::ffi::OsString;

    #[test]
    fn accepts_startup_window_without_disturbing_guest_inputs() {
        let options = parse_args(
            [
                "--warmup-seconds",
                "2",
                "--measure-seconds",
                "8",
                "--skip-guest-clock",
                "--volatile",
            ]
            .map(OsString::from),
        )
        .unwrap();

        assert_eq!(options.warmup, Duration::from_secs(2));
        assert_eq!(options.measure, Duration::from_secs(8));
        assert_eq!(options.sample_interval, Duration::from_millis(157));
        assert!(!options.guest_clock);
        assert_eq!(options.engine_args, [OsString::from("--volatile")]);
    }

    #[test]
    fn rejects_zero_measurement_windows() {
        assert!(parse_args([OsString::from("--measure-seconds"), OsString::from("0")]).is_err());
    }

    #[test]
    fn accepts_a_denser_cpu_sample_interval() {
        let options =
            parse_args(["--sample-interval-ms", "20", "--volatile"].map(OsString::from)).unwrap();

        assert_eq!(options.sample_interval, Duration::from_millis(20));
        assert_eq!(options.engine_args, [OsString::from("--volatile")]);
    }

    #[test]
    fn reads_cortex_m_exception_number_from_xpsr() {
        assert_eq!(
            super::exception_number("R15=80074070 XPSR=0100001f --T handler"),
            Some(31)
        );
        assert_eq!(
            super::exception_number("R15=80074070 XPSR=01000000 --T thread"),
            Some(0)
        );
        assert_eq!(super::exception_number("R15=80074070"), None);
    }
}
