use acadrust::io::dwg::DwgReader;
use acadrust::types::Handle;
use acadrust::entities::EntityType;
use acadrust::objects::ObjectType;

fn main() {
    let input = "acadrust_morki/1.dwg";
    
    let mut reader = DwgReader::from_file(input).expect("open");
    let doc = reader.read().expect("read");

    // Check INSERT 0x1EE1
    let insert_h = Handle::new(0x1EE1);
    println!("=== INSERT 0x1EE1 ===");
    for entity in doc.entities() {
        let c = entity.common();
        if c.handle == insert_h {
            println!("  type={} owner={:#X}", entity.as_entity().entity_type(), c.owner_handle.value());
            println!("  xdict={:?}", c.xdictionary_handle);
            println!("  reactors={:?}", c.reactors);
            if let EntityType::Insert(ins) = entity {
                println!("  block_name={}", ins.block_name);
                println!("  has_attribs={}", ins.has_attributes());
                println!("  attribs count={}", ins.attributes.len());
                for (i, att) in ins.attributes.iter().enumerate() {
                    println!("    attrib[{}]: handle={:#X} tag={}", i, att.common.handle.value(), att.tag);
                }
            }
            break;
        }
    }

    // Check DictionaryVar 0x17758
    let dv_h = Handle::new(0x17758);
    println!("\n=== DictionaryVar 0x17758 ===");
    if let Some(obj) = doc.objects.get(&dv_h) {
        if let ObjectType::DictionaryVariable(dv) = obj {
            println!("  owner={:#X} schema={} value={}", dv.owner_handle.value(), dv.schema_number, dv.value);
        } else {
            println!("  type={:?}", std::mem::discriminant(obj));
        }
    } else {
        println!("  NOT FOUND in objects");
    }

    // Check Dictionary 0x5E
    let dict_h = Handle::new(0x5E);
    println!("\n=== Dictionary 0x5E ===");
    if let Some(ObjectType::Dictionary(d)) = doc.objects.get(&dict_h) {
        println!("  handle={:#X} owner={:#X}", d.handle.value(), d.owner.value());
        println!("  entries ({}):", d.entries.len());
        for (name, h) in &d.entries {
            println!("    '{}' -> {:#X}", name, h.value());
        }
    } else {
        println!("  NOT FOUND or not a Dictionary");
    }

    // Check what handle 0x84, 0x227, 0x228 are
    for &h in &[0x84u64, 0x227, 0x228] {
        let handle = Handle::new(h);
        println!("\n=== Handle {:#X} ===", h);
        let mut found = false;
        if let Some(obj) = doc.objects.get(&handle) {
            println!("  object: {:?}", std::mem::discriminant(obj));
            match obj {
                ObjectType::Dictionary(d) => {
                    println!("  Dictionary: owner={:#X} entries={}", d.owner.value(), d.entries.len());
                }
                ObjectType::DictionaryVariable(dv) => {
                    println!("  DictVar: owner={:#X} value={}", dv.owner_handle.value(), dv.value);
                }
                _ => {}
            }
            found = true;
        }
        if !found {
            for entity in doc.entities() {
                if entity.common().handle == handle {
                    println!("  entity: {}", entity.as_entity().entity_type());
                    found = true;
                    break;
                }
            }
        }
        if !found {
            for br in doc.block_records.iter() {
                if br.handle == handle {
                    println!("  block_record: '{}'", br.name);
                    found = true;
                    break;
                }
            }
        }
        if !found {
            for lt in doc.line_types.iter() {
                if lt.handle == handle {
                    println!("  linetype: '{}'", lt.name);
                    found = true;
                    break;
                }
            }
        }
        if !found {
            for ly in doc.layers.iter() {
                if ly.handle == handle {
                    println!("  layer: '{}'", ly.name);
                    found = true;
                    break;
                }
            }
        }
        if !found {
            println!("  NOT FOUND");
        }
    }

    // Max handle in the document
    let mut max_handle = 0u64;
    for entity in doc.entities() {
        max_handle = max_handle.max(entity.common().handle.value());
    }
    for (h, _) in &doc.objects {
        max_handle = max_handle.max(h.value());
    }
    println!("\n=== Handle Space ===");
    println!("  Max handle in doc: {:#X}", max_handle);
}
