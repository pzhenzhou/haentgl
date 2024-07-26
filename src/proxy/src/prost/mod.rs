#[rustfmt::skip]
pub mod control_plane {
    include!("control_plane.rs");
}

#[rustfmt::skip]
#[allow(clippy::large_enum_variant)]
pub mod common_proto {
    include!("common_proto.rs");
}

#[rustfmt::skip]
pub mod topology {
    include!("topology.rs");
}
