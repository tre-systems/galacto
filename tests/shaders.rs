use naga::valid::{Capabilities, ValidationFlags, Validator};
use std::collections::BTreeSet;
use std::fs;

const SHADERS: &[(&str, &str)] = &[
    ("halo.wgsl", include_str!("../src/shaders/halo.wgsl")),
    ("post.wgsl", include_str!("../src/shaders/post.wgsl")),
    ("render.wgsl", include_str!("../src/shaders/render.wgsl")),
    ("update.wgsl", include_str!("../src/shaders/update.wgsl")),
];

#[test]
fn wgsl_shaders_parse_and_validate() {
    let listed: BTreeSet<_> = SHADERS.iter().map(|(name, _)| (*name).to_owned()).collect();
    let discovered: BTreeSet<_> = fs::read_dir("src/shaders")
        .expect("read src/shaders")
        .map(|entry| {
            entry
                .expect("read shader entry")
                .file_name()
                .into_string()
                .expect("shader filename is UTF-8")
        })
        .filter(|name| name.ends_with(".wgsl"))
        .collect();
    assert_eq!(
        listed, discovered,
        "SHADERS list must cover every WGSL file"
    );

    for (name, source) in SHADERS {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{name} failed to parse:\n{error}"));
        Validator::new(ValidationFlags::all(), Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|error| panic!("{name} failed validation:\n{error:?}"));
    }
}
