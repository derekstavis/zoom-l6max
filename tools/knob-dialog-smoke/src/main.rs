//! Firmware patch validation through GPIO encoders and firmware LCD frames.
use l6max_host::{engine::Options, headless::Headless, indicators::rings};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
fn capture(
    guest: &Headless,
    ms: u32,
    path: &Path,
) -> Result<[u8; 1024], Box<dyn std::error::Error>> {
    let end = guest.guest_ms() + ms;
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut last = None;
    while guest.guest_ms() < end {
        if let Some(frame) = guest.sample_display()? {
            last = Some(frame);
        }
        if Instant::now() > deadline {
            return Err("display timeout".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    if let Some(frame) = guest.sample_display()? {
        last = Some(frame);
    }
    let raw = last.ok_or("no display frame")?;
    image::DynamicImage::ImageLuma8(image::GrayImage::from_fn(128, 64, |x, y| {
        image::Luma([
            if raw[((63 - y as usize) / 8) * 128 + x as usize] & (1 << ((63 - y as usize) % 8)) != 0
            {
                255
            } else {
                0
            },
        ])
    }))
    .resize(768, 384, image::imageops::FilterType::Nearest)
    .save(path)?;
    Ok(raw)
}
fn expected_value(mode: u32, raw: u8, image: &[u8]) -> String {
    let table = |offset: usize| {
        f32::from_le_bytes(
            image[offset + raw as usize * 4..offset + raw as usize * 4 + 4]
                .try_into()
                .unwrap(),
        )
    };
    match mode {
        0 | 2 | 3 => {
            let db = table(0xa5138);
            if db > 0.0 {
                format!("+{db:.2} dB")
            } else {
                format!("{db:.2} dB")
            }
        }
        1 => {
            let hz = table(0xa5738);
            if hz < 1000.0 {
                format!("{hz:.0} Hz")
            } else {
                format!("{:.2} kHz", hz / 1000.0)
            }
        }
        8 => match raw {
            63 | 64 => "CENTER".into(),
            0..=62 => format!("L {}%", ((63 - raw as u32) * 100 + 31) / 63),
            _ => format!("R {}%", ((raw as u32 - 64) * 100 + 31) / 63),
        },
        _ => format!("{}%", (raw as u32 * 100 + 63) / 127),
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut engine_args = Vec::new();
    let mut installed = None;
    while let Some(arg) = args.next() {
        if arg == "--installed-package" {
            installed = Some(std::path::PathBuf::from(
                args.next().ok_or("--installed-package needs a path")?,
            ));
        } else {
            engine_args.push(arg);
        }
    }
    let options = Options::parse_args(engine_args)?;
    if options.state_dir.is_some() && installed.is_none() {
        return Err("use --volatile to load the supplied patched image".into());
    }
    let image = if let Some(package) = installed {
        let data = fs::read(package)?;
        l6max_host::package::check_layout(&data)?;
        l6max_host::update_bootloader::validate(&data)?;
        let length = u32::from_le_bytes(data[0x1a31f8..0x1a31fc].try_into().unwrap()) as usize;
        let image = data[0x200..0x200 + length].to_vec();
        let state = options
            .state_dir
            .as_ref()
            .ok_or("installed-package requires persistent state")?;
        let nor = fs::read(state.join("main-nor.bin"))?;
        if nor.get(0x50000..0x50000 + length) != Some(image.as_slice()) {
            return Err("persistent NOR does not contain the supplied patched package".into());
        }
        image
    } else {
        fs::read(options.firmware_dir.join("main_firmware.bin"))?
    };
    let guest = Headless::start(&options)?;
    guest.recorder()?;
    let inputs = guest.engine.inputs.as_ref().unwrap();
    let qmp = &guest.engine.management[0];
    println!("window: {:02x?}", qmp.read_memory(0, 0x80202610, 4)?);
    let before = rings(inputs.panel.snapshot().indicators);
    let original_resources = qmp.read_memory(0, 0x802059b8, 12)?;
    let original_bitmap = qmp.read_memory(0, 0x80207458, 4)?;
    let original_header = qmp.read_memory(0, 0x8020098c, 6)?;
    let original_value_y = qmp.read_memory(0, 0x8020099c, 2)?;
    let original_polarity = qmp.read_memory(0, 0x80200986, 2)?;
    let original_lengths_table =
        u32::from_le_bytes(qmp.read_memory(0, 0x802050fc, 4)?.try_into().unwrap());
    let original_lengths = qmp.read_memory(0, original_lengths_table + 229 * 2, 6)?;
    let mut original_text = Vec::new();
    for (pointer, capacity) in original_resources
        .chunks_exact(4)
        .zip(original_lengths.chunks_exact(2))
    {
        let address = u32::from_le_bytes(pointer.try_into().unwrap());
        let count = (usize::from(u16::from_le_bytes(capacity.try_into().unwrap())) + 1) * 2;
        original_text.push((address, qmp.read_memory(0, address, count)?));
    }
    let abandoned_page = qmp.read_memory(0, 0x841ff000, 4096)?;
    let heap = || -> Result<Vec<u32>, Box<dyn std::error::Error>> {
        Ok(qmp
            .read_memory(0, 0x8020ab1c, 16)?
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect())
    };
    assert_eq!(
        &image[0x40c..0x410],
        &0x8007eea9u32.to_le_bytes(),
        "startup hook still reserves patch RAM"
    );
    let text_resource = |index: u32| -> Result<String, Box<dyn std::error::Error>> {
        let pointer = u32::from_le_bytes(
            qmp.read_memory(0, 0x802059b8 + 4 * index, 4)?
                .try_into()
                .unwrap(),
        );
        let bytes = qmp.read_memory(0, pointer, 48)?;
        let text = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|c| *c != 0)
            .collect::<Vec<_>>();
        Ok(String::from_utf16(&text)?)
    };
    let mut reports = 0;
    for (mode, button, name) in [
        (9, 35, "level"),
        (0, 28, "high"),
        (1, 33, "freq"),
        (2, 38, "mid"),
        (3, 43, "low"),
        (4, 10, "aux1"),
        (5, 15, "aux2"),
        (6, 20, "efx"),
        (7, 25, "sub-mix"),
        (8, 30, "pan"),
    ] {
        guest.tap(button, 50, 450)?;
        let baseline = capture(
            &guest,
            150,
            &options.logs.join(format!("{name}-before.png")),
        )?;
        let heap_before = heap()?;
        inputs.encoder(0, -3)?;
        reports += 3;
        let frame = capture(&guest, 700, &options.logs.join(format!("{name}.png")))?;
        println!(
            "{name} mode {mode}: state {:02x?}",
            qmp.read_memory(0, 0x8020a2c0, 6)?
        );
        assert!(frame.iter().any(|b| *b != 0));
        let state = qmp.read_memory(0, 0x8020a2c0, 1)?;
        assert_eq!(state[0], 1, "knob did not show firmware notification");
        let heap_live = heap()?;
        assert_eq!(
            heap_live[2],
            heap_before[2] + 1,
            "popup must allocate once through firmware heap"
        );
        assert_eq!(heap_live[3], heap_before[3]);
        let allocation = u32::from_le_bytes(qmp.read_memory(0, 0x80207458, 4)?.try_into().unwrap());
        assert!(
            (0x808a4150..0x80921140).contains(&allocation),
            "popup state outside firmware heap"
        );
        let allocated_size =
            u32::from_le_bytes(qmp.read_memory(0, allocation - 4, 4)?.try_into().unwrap());
        assert_ne!(
            allocated_size & 0x80000000,
            0,
            "allocator does not own popup block"
        );
        assert_eq!(
            qmp.read_memory(0, allocation + 8, 4)?,
            0x4b4e4f42u32.to_le_bytes()
        );
        assert_eq!(
            text_resource(0)?,
            name.to_uppercase(),
            "wrong selected setting title"
        );
        let offset = [2, 5, 4, 6, 15, 16, 12, 9, 8, 0x120][mode as usize];
        let raw = qmp.read_memory(0, 0x8045ff84 + offset, 1)?[0];
        assert_eq!(
            text_resource(1)?,
            expected_value(mode, raw, &image),
            "display differs from mixer state"
        );
        assert_eq!(text_resource(2)?, "", "channel line still visible");
        assert_eq!(
            qmp.read_memory(0, 0x8020099c, 2)?,
            32u16.to_le_bytes(),
            "value not vertically centered"
        );
        inputs.encoder(0, 2)?;
        reports += 2;
        let changed = capture(
            &guest,
            500,
            &options.logs.join(format!("{name}-changed.png")),
        )?;
        assert_ne!(frame, changed, "value did not update while popup visible");
        let raw = qmp.read_memory(0, 0x8045ff84 + offset, 1)?[0];
        assert_eq!(text_resource(1)?, expected_value(mode, raw, &image));
        assert_eq!(
            heap()?[2],
            heap_live[2],
            "popup refresh allocated another block"
        );
        guest.wait(2300)?;
        assert_eq!(
            qmp.read_memory(0, 0x8020a2c0, 1)?[0],
            0,
            "popup did not expire"
        );
        let expired = capture(
            &guest,
            150,
            &options.logs.join(format!("{name}-expired.png")),
        )?;
        // The recorder's top band is static here. Checking notification state
        // alone misses glyphs left on layer 6 after restoring widget geometry.
        for y in 9..16usize {
            for x in 9..119usize {
                let index = ((63 - y) / 8) * 128 + x;
                let mask = 1 << ((63 - y) % 8);
                assert_eq!(
                    expired[index] & mask,
                    baseline[index] & mask,
                    "{name}: popup title left a pixel at ({x}, {y}) after expiry"
                );
            }
        }
        assert_eq!(
            qmp.read_memory(0, 0x802059b8, 12)?,
            original_resources,
            "original Done resources not restored"
        );
        assert_eq!(
            qmp.read_memory(0, 0x80207458, 4)?,
            original_bitmap,
            "Done bitmap not restored"
        );
        assert_eq!(
            qmp.read_memory(0, 0x8020098c, 6)?,
            original_header,
            "Done layout not restored"
        );
        assert_eq!(qmp.read_memory(0, 0x8020099c, 2)?, original_value_y);
        assert_eq!(
            qmp.read_memory(0, 0x80200986, 2)?,
            original_polarity,
            "Done text polarity not restored"
        );
        let heap_after = heap()?;
        assert_eq!(
            heap_after[3],
            heap_before[3] + 1,
            "popup allocation was not released"
        );
        assert_eq!(heap_after[0], heap_before[0], "popup leaked heap bytes");
        assert_eq!(
            qmp.read_memory(0, original_lengths_table + 229 * 2, 6)?,
            original_lengths,
            "Done buffer capacities were not restored"
        );
        for (address, bytes) in &original_text {
            assert_eq!(
                qmp.read_memory(0, *address, bytes.len())?,
                *bytes,
                "original Done text was overwritten"
            );
        }
    }
    // Blue-strip buttons and further encoder events must pass through the
    // notification, without waiting for its timeout or pressing Confirm.
    guest.tap(35, 50, 100)?;
    inputs.encoder(1, -1)?;
    reports += 1;
    capture(&guest, 400, &options.logs.join("channel-2-level.png"))?;
    assert_eq!(text_resource(0)?, "LEVEL");
    guest.tap(28, 50, 100)?;
    inputs.encoder(1, -1)?;
    reports += 1;
    capture(&guest, 400, &options.logs.join("channel-2-high.png"))?;
    assert_eq!(text_resource(0)?, "HIGH");
    guest.wait(2300)?;
    assert_eq!(qmp.read_memory(0, 0x802059b8, 12)?, original_resources);
    let actual = fs::read_to_string(options.logs.join("engine.trace"))?
        .matches("l6 panel USART1 TX a1\n")
        .count();
    assert_eq!(actual, reports, "duplicate or missing encoder reports");
    assert_ne!(before[0], rings(inputs.panel.snapshot().indicators)[0]);
    assert_eq!(
        qmp.read_memory(0, 0x841ff000, 4096)?,
        abandoned_page,
        "patch wrote abandoned RAM page"
    );
    println!("All ten modes: popup updates, guest-timed expiry, {actual} exact encoder reports.");
    Ok(())
}
