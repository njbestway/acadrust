use acadrust::io::dwg::{DwgReader, DwgWriter};
use std::path::Path;

fn roundtrip(input: &str) {
    let stem = Path::new(input).file_stem().unwrap().to_str().unwrap();
    let output = format!("target/roundtrip_test/{}_roundtrip.dwg", stem);

    print!("{:<40}", input);

    let mut reader = match DwgReader::from_file(input) {
        Ok(r) => r,
        Err(e) => { println!("READ-OPEN FAIL: {}", e); return; }
    };
    let doc = match reader.read() {
        Ok(d) => d,
        Err(e) => { println!("READ FAIL: {}", e); return; }
    };

    let ent = doc.entity_count();
    let lay = doc.layers.len();
    let lt = doc.line_types.len();
    let obj = doc.objects.len();
    let blk = doc.block_records.len();

    if let Err(e) = DwgWriter::write_to_file(&output, &doc) {
        println!("WRITE FAIL: {}", e);
        return;
    }

    let mut r2 = match DwgReader::from_file(&output) {
        Ok(r) => r,
        Err(e) => { println!("RE-OPEN FAIL: {}", e); return; }
    };
    let rt = match r2.read() {
        Ok(d) => d,
        Err(e) => { println!("RE-READ FAIL: {}", e); return; }
    };

    let ok = rt.entity_count() == ent && rt.layers.len() == lay
        && rt.line_types.len() == lt && rt.objects.len() == obj
        && rt.block_records.len() == blk;

    println!("ent={:<6} lay={:<3} lt={:<3} obj={:<4} blk={:<5} | rt ent={:<6} lay={:<3} lt={:<3} obj={:<4} blk={:<5} {}",
        ent, lay, lt, obj, blk,
        rt.entity_count(), rt.layers.len(), rt.line_types.len(), rt.objects.len(), rt.block_records.len(),
        if ok { "OK" } else { "MISMATCH" });
}

fn main() {
    let files = ["1.dwg", "2.dwg", "3.dwg", "4.dwg"];
    for f in &files {
        let path = format!("acadrust_morki/{}", f);
        roundtrip(&path);
    }
}
