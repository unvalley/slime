use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=SLIME_SOURCE_REVISION");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let Ok(revision) = env::var("SLIME_SOURCE_REVISION") else {
        // Cross-target `cargo check` does not have a Windows resource compiler.
        // Release packaging rejects Windows payloads without this metadata.
        return;
    };
    assert!(
        revision.len() == 40
            && revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "SLIME_SOURCE_REVISION must be a lowercase 40-character Git revision"
    );

    let mut resource = winresource::WindowsResource::new();
    resource
        .set("Comments", &format!("Source revision: {revision}"))
        .set("FileDescription", "Slime conversion engine")
        .set("OriginalFilename", "slime_ffi.dll")
        .set_version_info(winresource::VersionInfo::FILETYPE, 2);
    resource
        .compile()
        .expect("failed to embed Windows source revision metadata");
}
