use acadrust::io::import::collada::ColladaImporter;
use acadrust::DwgWriter;

fn main() -> acadrust::error::Result<()> {
    let dae_path = "examples/dae/sample.dae";
    let dwg_path = "target/sample_dae.dwg";

    println!("Importing COLLADA: {}", dae_path);
    let importer = ColladaImporter::from_file(dae_path)?;
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
