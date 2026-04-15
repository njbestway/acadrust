//! Diagnostic: Dump dictionary entries and validate referenced objects.
//!
//! For each Dictionary in the document, lists entry names → handle,
//! whether the handle exists in document.objects, and the object type.
//!
//! Usage:
//!   cargo run --example diag_dict_entries -- <file.dwg> [dict_handle_hex]
//!
//! Examples:
//!   cargo run --example diag_dict_entries -- tests/roundtrip/sample.dwg
//!   cargo run --example diag_dict_entries -- tests/roundtrip/sample.dwg 92

use std::io::Cursor;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <file.dwg> [dict_handle_hex]", args[0]);
        std::process::exit(1);
    }
    let path = &args[1];
    let filter_handle: Option<u64> = args.get(2).map(|s| {
        u64::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16)
            .expect("invalid hex handle")
    });

    let data = std::fs::read(path).expect("read file");
    let mut reader = acadrust::DwgReader::from_stream(Cursor::new(&data));
    let doc = reader.read().expect("parse DWG");

    println!("Version: {:?}", doc.version);
    println!("Objects in document: {}", doc.objects.len());
    println!("Entities in document: {}", doc.entities().count());
    println!();

    // Collect all object types for quick lookup display
    let type_name = |h: &acadrust::types::Handle| -> String {
        match doc.objects.get(h) {
            None => "[NOT IN OBJECTS]".to_string(),
            Some(obj) => format!("{}", object_type_name(obj)),
        }
    };

    // Iterate all dictionaries
    let mut dicts: Vec<_> = doc.objects.iter().collect();
    dicts.sort_by_key(|(h, _)| h.value());

    for (handle, obj) in &dicts {
        let (entries, dict_type) = match obj {
            acadrust::objects::ObjectType::Dictionary(d) => {
                (d.entries.as_slice(), "Dictionary")
            }
            acadrust::objects::ObjectType::DictionaryWithDefault(d) => {
                (d.entries.as_slice(), "DictionaryWithDefault")
            }
            _ => continue,
        };

        if let Some(fh) = filter_handle {
            if handle.value() != fh {
                continue;
            }
        }

        let xdic = match obj {
            acadrust::objects::ObjectType::Dictionary(d) => d.xdictionary_handle,
            _ => None,
        };

        println!("── {} handle={:#X} ({} entries) ──", dict_type, handle.value(), entries.len());
        if let Some(xd) = xdic {
            let exists = doc.objects.contains_key(&xd);
            println!("  xdictionary: {:#X} (exists={})", xd.value(), exists);
        }

        let mut invalid_count = 0;
        for (name, entry_handle) in entries {
            let exists = !entry_handle.is_null() && doc.objects.contains_key(entry_handle);
            let tname = if entry_handle.is_null() {
                "NULL".to_string()
            } else {
                type_name(entry_handle)
            };
            let marker = if !entry_handle.is_null() && !exists { " *** MISSING ***" } else { "" };
            if !exists && !entry_handle.is_null() {
                invalid_count += 1;
            }
            println!("  {:40} → {:#06X}  {}{}",
                name, entry_handle.value(), tname, marker);
        }
        if invalid_count > 0 {
            println!("  ** {} entries reference missing objects **", invalid_count);
        }
        println!();
    }

    // Also check: does handle 0xF2EA exist anywhere?
    if let Some(fh) = filter_handle {
        // If a specific dict was requested, also look for the entries' handles
    } else {
        // Check some specific handles
        for test_h in [0x92u64, 0xF2EA] {
            let h = acadrust::types::Handle::new(test_h);
            print!("Handle {:#X}: ", test_h);
            match doc.objects.get(&h) {
                Some(obj) => println!("{}", object_type_name(obj)),
                None => println!("NOT FOUND in objects"),
            }
        }
    }
}

fn object_type_name(obj: &acadrust::objects::ObjectType) -> &'static str {
    match obj {
        acadrust::objects::ObjectType::Dictionary(_) => "Dictionary",
        acadrust::objects::ObjectType::DictionaryWithDefault(_) => "DictionaryWithDefault",
        acadrust::objects::ObjectType::DictionaryVariable(_) => "DictionaryVariable",
        acadrust::objects::ObjectType::Layout(_) => "Layout",
        acadrust::objects::ObjectType::XRecord(_) => "XRecord",
        acadrust::objects::ObjectType::Group(_) => "Group",
        acadrust::objects::ObjectType::MLineStyle(_) => "MLineStyle",
        acadrust::objects::ObjectType::MultiLeaderStyle(_) => "MultiLeaderStyle",
        acadrust::objects::ObjectType::ImageDefinition(_) => "ImageDefinition",
        acadrust::objects::ObjectType::ImageDefinitionReactor(_) => "ImageDefReactor",
        acadrust::objects::ObjectType::PlotSettings(_) => "PlotSettings",
        acadrust::objects::ObjectType::Scale(_) => "Scale",
        acadrust::objects::ObjectType::SortEntitiesTable(_) => "SortEntitiesTable",
        acadrust::objects::ObjectType::RasterVariables(_) => "RasterVariables",
        acadrust::objects::ObjectType::BookColor(_) => "BookColor",
        acadrust::objects::ObjectType::PlaceHolder(_) => "PlaceHolder",
        acadrust::objects::ObjectType::WipeoutVariables(_) => "WipeoutVariables",
        acadrust::objects::ObjectType::GeoData(_) => "GeoData",
        acadrust::objects::ObjectType::SpatialFilter(_) => "SpatialFilter",
        acadrust::objects::ObjectType::VisualStyle(_) => "VisualStyle",
        acadrust::objects::ObjectType::Material(_) => "Material",
        acadrust::objects::ObjectType::TableStyle(_) => "TableStyle",
        acadrust::objects::ObjectType::Unknown { type_name, .. } => {
            // Leak a str for the 'static lifetime (fine for diagnostics)
            Box::leak(type_name.clone().into_boxed_str())
        }
    }
}
