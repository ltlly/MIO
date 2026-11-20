fn main() {
    println!("cargo:rerun-if-changed=assets/mio.rc");
    println!("cargo:rerun-if-changed=assets/mio.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/mio.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("failed to compile Windows icon resource");
    }
}
