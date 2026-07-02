use acadrust::entities::EntityType;
use acadrust::{CadDocument, DwgReader, DwgWriter};

fn solids(doc: &CadDocument) -> Vec<(u64, bool, usize, Vec<u8>)> {
    doc.entities().filter_map(|e| if let EntityType::Solid3D(s) = e {
        let a = &s.acis_data;
        Some((s.common.handle.value(), a.is_binary, a.sab_data.len(),
              a.sab_data.iter().take(24).copied().collect()))
    } else { None }).collect()
}

fn main() {
    let src = std::env::args().nth(1).unwrap();
    let mut r = DwgReader::from_file(&src).unwrap();
    let doc = r.read().unwrap();
    let before = solids(&doc);
    println!("READ1: {} solids", before.len());

    let tmp = "/tmp/claude-1000/-home-hakanseven-Kodlama-OpenCADStudio/bb995f39-5eec-49ae-a154-c3ce1e400b45/scratchpad/bytecheck_rt.dwg";
    DwgWriter::write_to_file(tmp, &doc).unwrap();
    let mut r2 = DwgReader::from_file(tmp).unwrap();
    let doc2 = r2.read().unwrap();
    let after = solids(&doc2);
    println!("READ2: {} solids", after.len());

    let n = before.len().min(after.len());
    let (mut len_diff, mut byte_diff, mut ok) = (0, 0, 0);
    for i in 0..n {
        let (h, _b1, l1, p1) = &before[i];
        let (_h2, _b2, l2, p2) = &after[i];
        if l1 != l2 { len_diff += 1; if len_diff <= 3 { println!("  solid[{i}] h={h}: len {l1} -> {l2}"); } }
        else if p1 != p2 { byte_diff += 1; if byte_diff <= 3 { println!("  solid[{i}] h={h}: len OK ({l1}) but first24 differ\n    b={:02x?}\n    a={:02x?}", p1, p2); } }
        else { ok += 1; }
    }
    println!("SUMMARY: ok={ok} len_diff={len_diff} byte_diff={byte_diff} (of {n})");
    println!("total sab bytes before={} after={}", before.iter().map(|x|x.2).sum::<usize>(), after.iter().map(|x|x.2).sum::<usize>());
}
