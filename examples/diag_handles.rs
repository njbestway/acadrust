use acadrust::io::dwg::DwgReader;
fn main() {
    let path = "acadrust_morki/General.dwg";
    let mut reader = DwgReader::from_file(path).expect("open");
    let doc = reader.read().expect("read");

    let check_handles: Vec<u64> = vec![0x22, 0x89, 0x1B7, 0x1B8];
    
    println!("=== Checking handles from AutoCAD errors ===\n");
    
    for h in &check_handles {
        let handle = acadrust::types::Handle::from(*h);
        
        // Check in objects
        if let Some(obj) = doc.objects.get(&handle) {
            println!("Handle 0x{:X}: OBJECT = {:?}", h, std::mem::discriminant(obj));
        }
        
        // Check in text styles
        for ts in doc.text_styles.iter() {
            if ts.handle.value() == *h {
                println!("Handle 0x{:X}: TextStyle '{}'", h, ts.name);
            }
        }
        
        // Check in layers
        for l in doc.layers.iter() {
            if l.handle.value() == *h {
                println!("Handle 0x{:X}: Layer '{}'", h, l.name);
            }
        }
        
        // Check in line types
        for lt in doc.line_types.iter() {
            if lt.handle.value() == *h {
                println!("Handle 0x{:X}: LineType '{}'", h, lt.name);
            }
        }
        
        // Check in dim styles
        for ds in doc.dim_styles.iter() {
            if ds.handle.value() == *h {
                println!("Handle 0x{:X}: DimStyle '{}'", h, ds.name);
            }
        }
        
        // Check in block records
        for br in doc.block_records.iter() {
            if br.handle.value() == *h {
                println!("Handle 0x{:X}: BlockRecord '{}'", h, br.name);
            }
        }
        
        // Check in vports
        for v in doc.vports.iter() {
            if v.handle.value() == *h {
                println!("Handle 0x{:X}: VPort '{}'", h, v.name);
            }
        }
        
        // Check in app ids
        for a in doc.app_ids.iter() {
            if a.handle.value() == *h {
                println!("Handle 0x{:X}: AppId '{}'", h, a.name);
            }
        }
    }
}
