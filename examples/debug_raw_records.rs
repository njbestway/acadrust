/// Compare raw object records between original and roundtripped DWG files.
/// For each failing LwPolyline handle, extract the bytes and check CRC.
use acadrust::io::dwg::crc;
use acadrust::io::dwg::dwg_stream_readers::handle_reader;
use acadrust::{DwgReader, DwgWriter};

fn main() {
    let original_path = "tests/issue14/General.dwg";
    let roundtrip_path = "target/General_roundtrip_debug.dwg";

    // Generate roundtrip file
    println!("Generating roundtrip...");
    let mut reader = DwgReader::from_file(original_path).expect("open");
    let doc = reader.read().expect("read");
    DwgWriter::write_to_file(roundtrip_path, &doc).expect("write");
    println!("Done. File size: {} bytes", std::fs::metadata(roundtrip_path).unwrap().len());

    // Failing handles from BricsCAD
    let failing: Vec<u64> = vec![
        0x2413D, 0x2413F, 0x24156, 0x24186, 0x24189, 0x2418C,
    ];
    // Passing handles in the same range for comparison
    let passing: Vec<u64> = vec![
        0x2410A, 0x2411F, 0x24122, 0x24125, 0x24128,
    ];

    println!("=== ORIGINAL FILE ===");
    dump_records(original_path, &failing, &passing);

    println!("\n\n=== ROUNDTRIPPED FILE ===");
    dump_records(roundtrip_path, &failing, &passing);
}

fn dump_records(path: &str, failing: &[u64], passing: &[u64]) {
    let mut reader = DwgReader::from_file(path).expect("Failed to open");
    let info = reader.read_file_header().expect("Failed to read header");

    let objects_buf = reader
        .get_section_buffer("AcDb:AcDbObjects", &info)
        .expect("Failed to read objects section");
    let handles_buf = reader
        .get_section_buffer("AcDb:Handles", &info)
        .expect("Failed to read handles section");

    let handle_map = handle_reader::read_handles(&handles_buf)
        .expect("Failed to parse handle map");

    // AC18: offsets may need base adjustment (but for AC18 they're relative to section start)
    println!("  Objects section size: {} bytes", objects_buf.len());
    println!("  Handle map entries: {}", handle_map.len());

    println!("\n  --- FAILING HANDLES ---");
    for &h in failing {
        dump_one_record(h, &handle_map, &objects_buf, true);
    }

    println!("\n  --- PASSING HANDLES ---");
    for &h in passing {
        dump_one_record(h, &handle_map, &objects_buf, true);
    }
}

fn dump_one_record(
    handle: u64,
    handle_map: &std::collections::HashMap<u64, i64>,
    data: &[u8],
    verbose: bool,
) {
    let offset = match handle_map.get(&handle) {
        Some(&o) => o as usize,
        None => {
            println!("  Handle {:#X}: NOT IN HANDLE MAP", handle);
            return;
        }
    };

    if offset >= data.len() {
        println!("  Handle {:#X}: offset {} out of range (data len {})", handle, offset, data.len());
        return;
    }

    // Parse the record framing
    let mut pos = offset;

    // Read MS (modular short) = data size
    let (size, ms_len) = read_modular_short(&data[pos..]);
    pos += ms_len;

    // Read MC (modular char) = handle bits (R2010+)
    let (handle_bits, mc_len) = read_modular_char(&data[pos..]);
    pos += mc_len;

    // Merged data
    let data_end = pos + size;
    if data_end > data.len() {
        println!("  Handle {:#X}: data extends past section (offset={}, size={}, section_len={})",
            handle, offset, size, data.len());
        return;
    }

    // CRC was computed over: [MS bytes][MC bytes][merged data]
    let crc_region = &data[offset..data_end];
    let expected_crc = crc::crc16(crc::CRC16_SEED, crc_region);

    // Read stored CRC (2 bytes LE after merged data)
    let stored_crc = if data_end + 2 <= data.len() {
        u16::from_le_bytes([data[data_end], data[data_end + 1]])
    } else {
        println!("  Handle {:#X}: CRC bytes out of range", handle);
        return;
    };

    let crc_ok = expected_crc == stored_crc;

    // Read first few bytes to identify object type
    let merged = &data[pos..data_end];
    let type_code = if !merged.is_empty() {
        // R2010+ compact type encoding: BB + 1 or 2 bytes
        let first_byte = merged[0];
        let bb = (first_byte >> 6) & 0x03;
        match bb {
            0 => {
                // BB=00: next byte is the type code (but it's in the bit stream)
                // For a proper decode we'd need the bit reader
                format!("BB=00 (compact, byte starts {:02X})", first_byte)
            }
            1 => format!("BB=01 (compact, byte starts {:02X})", first_byte),
            2 => format!("BB=10 (compact, byte starts {:02X})", first_byte),
            3 => format!("BB=11 (compact, byte starts {:02X})", first_byte),
            _ => unreachable!(),
        }
    } else {
        "EMPTY".to_string()
    };

    println!(
        "  Handle {:#06X}: offset={:>8}, MS_len={}, size={:>4}, MC_len={}, hbits={:>3}, CRC={} (exp={:#06X} got={:#06X}) type={}",
        handle, offset, ms_len, size, mc_len, handle_bits,
        if crc_ok { "OK" } else { "FAIL" },
        expected_crc, stored_crc,
        type_code
    );

    if verbose {
        // Print first 32 bytes of merged data
        let show_len = size.min(32);
        let hex: Vec<String> = merged[..show_len].iter().map(|b| format!("{:02X}", b)).collect();
        println!("    data[0..{}]: {}", show_len, hex.join(" "));

        // Print last 8 bytes
        if size > 32 {
            let start = size - 8;
            let hex: Vec<String> = merged[start..].iter().map(|b| format!("{:02X}", b)).collect();
            println!("    data[{}..{}]: {}", start, size, hex.join(" "));
        }
    }
}

fn read_modular_short(data: &[u8]) -> (usize, usize) {
    let mut value: usize = 0;
    let mut shift = 0;
    let mut i = 0;
    loop {
        if i + 1 >= data.len() { break; }
        let word = u16::from_le_bytes([data[i], data[i + 1]]);
        i += 2;
        value |= ((word & 0x7FFF) as usize) << shift;
        shift += 15;
        if (word & 0x8000) == 0 { break; }
    }
    (value, i)
}

fn read_modular_char(data: &[u8]) -> (usize, usize) {
    let mut value: usize = 0;
    let mut shift = 0;
    let mut i = 0;
    loop {
        if i >= data.len() { break; }
        let b = data[i];
        i += 1;
        value |= ((b & 0x7F) as usize) << shift;
        shift += 7;
        if (b & 0x80) == 0 { break; }
    }
    (value, i)
}
