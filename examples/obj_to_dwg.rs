use acadrust::io::import::obj::ObjImporter;
use acadrust::DwgWriter;

fn main() -> acadrust::error::Result<()> {
    let obj_path = "examples/obj/sample.obj";
    let dwg_path = "target/sample_obj.dwg";

    println!("Importing OBJ: {}", obj_path);
    let importer = ObjImporter::from_file(obj_path)?;
    let doc = importer.import()?;

    let entity_count = doc.entities().count();
    println!("Imported {} mesh entities", entity_count);

    for entity in doc.entities() {
        if let acadrust::entities::EntityType::Mesh(mesh) = entity {
            println!(
                "  Mesh on layer '{}': {} vertices, {} faces",
                mesh.common.layer,
                mesh.vertices.len(),
                mesh.faces.len()
            );
        }
    }

    println!("Writing DWG: {}", dwg_path);
    DwgWriter::write_to_file(dwg_path, &doc)?;
    println!("Done!");

    Ok(())
}
