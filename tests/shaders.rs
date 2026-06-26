use naga::valid::{Capabilities, ValidationFlags, Validator};

const SHADERS: &[(&str, &str)] = &[
    ("halo.wgsl", include_str!("../src/shaders/halo.wgsl")),
    ("post.wgsl", include_str!("../src/shaders/post.wgsl")),
    ("render.wgsl", include_str!("../src/shaders/render.wgsl")),
    ("update.wgsl", include_str!("../src/shaders/update.wgsl")),
];

#[test]
fn wgsl_shaders_parse_and_validate() {
    for (name, source) in SHADERS {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{name} failed to parse:\n{error}"));
        Validator::new(ValidationFlags::all(), Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|error| panic!("{name} failed validation:\n{error:?}"));
    }
}
