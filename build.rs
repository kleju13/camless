//! Puts the app's icon into camless.exe too, so Explorer and Task Manager show it.

fn main() {
    println!("cargo:rerun-if-changed=camless.rc");
    println!("cargo:rerun-if-changed=gui/camless.ico");
    embed_resource::compile("camless.rc", embed_resource::NONE).manifest_optional().unwrap();
}
