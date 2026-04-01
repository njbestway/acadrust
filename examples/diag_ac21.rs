use acadrust::io::dwg::{DwgReader, DwgWriter};

fn dump_metadata(label: &str, path: &str) {
    let mut reader = DwgReader::from_file(path).expect("open");
    let info = reader.read_file_header().expect("header");
    if let Some(meta) = &info.ac21_metadata {
        println!("=== {} ===", label);
        println!("Pages Map CRC Seed: {:#018X}", meta.pages_map_crc_seed);
        println!("Sections Map CRC Seed: {:#018X}", meta.sections_map_crc_seed);
        println!("CRC Seed: {:#018X}", meta.crc_seed);
        println!("Page records: {}", info.page_records.len());
        println!("Section descriptors: {}", info.section_descriptors.len());
        for sd in &info.section_descriptors {
            if sd.encoding == 1 || sd.pages.len() <= 3 {
                println!("  Section: {} (encoding={}, pages={})", sd.name, sd.encoding, sd.pages.len());
                for (i, p) in sd.pages.iter().enumerate() {
                    let page_num = p.page_number as i32;
                    let page_info = info.page_records.get(&page_num);
                    let (offset, file_size) = page_info.map(|&(o, s)| (o, s)).unwrap_or((-1, -1));
                    println!("    page[{}]: id={}, comp={}, decomp={}, file_size={}, file_offset={}",
                        i, p.page_number, p.compressed_size, p.decompressed_size, file_size, offset);
                }
            }
        }
    } else {
        println!("{}: Not AC21 format", label);
    }
}

fn main() {
    let input = "acadrust_morki/4.dwg";
    let output = "target/4_roundtrip.dwg";

    dump_metadata("ORIGINAL", input);

    // Roundtrip
    let mut reader = DwgReader::from_file(input).expect("open");
    let doc = reader.read().expect("read");
    DwgWriter::write_to_file(output, &doc).expect("write");

    dump_metadata("ROUNDTRIPPED", output);
}
