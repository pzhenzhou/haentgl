#[allow(clippy::large_enum_variant)]
pub mod common_proto {
    include!("codegen/topology/common_proto.rs");
}

pub mod topology {
    include!("codegen/topology/topology.rs");
}