// Prost-generated protobuf types (populated by build.rs).
// Module hierarchy must match proto package paths for cross-references.
#![allow(dead_code)]
#![allow(clippy::large_enum_variant)]
// Doc comments are copied verbatim from the .proto sources and are not
// subject to our doc-formatting lints; suppress clippy doc lints for the
// mechanically generated bindings so upstream comment style can't fail CI.
#![allow(clippy::doc_overindented_list_items)]
#![allow(clippy::doc_lazy_continuation)]

pub mod common {
    pub mod v1 {
        include!("gdx.common.v1.rs");
    }
}

pub mod edge {
    pub mod v1 {
        include!("gdx.edge.v1.rs");
    }
}

pub mod health {
    pub mod v1 {
        include!("gdx.health.v1.rs");
    }
}

pub mod sequencer {
    pub mod v1 {
        include!("gdx.sequencer.v1.rs");
    }
}
