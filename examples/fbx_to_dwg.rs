use acadrust::io::import::fbx::FbxImporter;
use acadrust::DwgWriter;

fn main() -> acadrust::error::Result<()> {
    let fbx_path = "examples/fbx/sample.fbx";
    let dwg_path = "target/sample_fbx.dwg";

    println!("Importing FBX: {}", fbx_path);
    let importer = FbxImporter::from_file(fbx_path)?;
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
