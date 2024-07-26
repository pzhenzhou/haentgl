use indexmap::IndexMap;
use std::fs;
use std::path::PathBuf;

const DEBUG: bool = false;

macro_rules! build_debug {
    ($($tokens: tt)*) => {
        if DEBUG {
            println!("cargo:warning={}", format!($($tokens)*))
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_files_rewrite_mapping = [
        ("control_plane.proto", vec![]),
        ("common.proto", vec![]),
        ("topology.proto", vec!["common_proto.rs", "topology.rs"]),
    ]
    .into_iter()
    .collect::<IndexMap<_, _>>();

    let output_dirs = ["./src/prost/", "./src/prost", "./src/prost/"];

    for (idx, entry) in proto_files_rewrite_mapping.iter().enumerate() {
        let proto = *entry.0;
        let proto_file = format!("{}/{}", "protos", proto);
        let output_dir = PathBuf::from(output_dirs[idx]);
        fs::create_dir_all(&output_dir)?;
        //let file_descriptor_set_path: PathBuf = output_dir.join("file_descriptor_set.bin");

        tonic_build::configure()
            //.file_descriptor_set_path(file_descriptor_set_path.as_path())
            .protoc_arg("--experimental_allow_proto3_optional")
            .type_attribute(".", "#[allow(non_camel_case_types)]")
            .type_attribute("common_proto.DBLocation", "#[derive(Eq, Hash)]")
            .type_attribute("common_proto.Endpoint", "#[derive(Eq, Hash)]")
            .type_attribute("common_proto.ClusterName", "#[derive(Eq, Hash)]")
            .type_attribute("common_proto.Cluster", "#[derive(Eq, Hash)]")
            .type_attribute("common_proto.ServiceExpose", "#[derive(Eq, Hash)]")
            .type_attribute(
                "common_proto.TenantKey",
                "#[derive(Eq, Hash, serde::Serialize, serde::Deserialize)]",
            )
            .out_dir(output_dir.as_path())
            .compile(&[proto_file], &["protos"])
            .unwrap_or_else(|_| panic!("Failed to compile protobuf files! {}", proto));

        let rewrite_files = entry.1;
        rewrite_files.iter().for_each(|rewrite_file| {
            let proto_rs = output_dir.join(rewrite_file);
            rewrite_proto_file(&proto_rs).expect("Failed to rewrite proto file!");
        });
    }
    Ok(())
}

fn rewrite_proto_file(proto_rs: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    build_debug!("Rewrite for {:?}", proto_rs);
    let mut generate_file = String::from_utf8(fs_err::read(proto_rs).unwrap()).unwrap();
    generate_file = generate_file.replace("DbLocation", "DBLocation");
    generate_file = generate_file.replace("DbService", "DBService");
    generate_file = generate_file.replace("DbServiceList", "DBServiceList");

    fs_err::write(proto_rs, generate_file)
        .unwrap_or_else(|_| panic!("Failed to write file {:?}", proto_rs));
    Ok(())
}
