//! Puts camless.ico into camless-gui.exe as icon 1: Explorer and the taskbar show
//! it, and the tray and the window load it from there.

fn main() {
    println!("cargo:rerun-if-changed=camless.rc");
    println!("cargo:rerun-if-changed=camless.ico");
    embed_resource::compile("camless.rc", embed_resource::NONE).manifest_optional().unwrap();
}
