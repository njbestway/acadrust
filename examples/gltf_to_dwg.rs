use acadrust::io::import::gltf::GltfImporter;
use acadrust::DwgWriter;

fn main() -> acadrust::error::Result<()> {
    let gltf_path = "examples/gltf/scene.gltf";
    let dwg_path = "target/scene_gltf.dwg";

    println!("Importing glTF: {}", gltf_path);
    let importer = GltfImporter::from_file(gltf_path)?;
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
