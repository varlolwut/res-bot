use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_directory =
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must provide CARGO_MANIFEST_DIR");
    let manifest_path = PathBuf::from(manifest_directory).join("res-bot.manifest");

    println!("cargo:rerun-if-changed={}", manifest_path.display());
    println!("cargo:rerun-if-env-changed=PROFILE");
    let profile = env::var("PROFILE").expect("Cargo must provide PROFILE");
    if profile != "release" {
        return;
    }
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");
    println!(
        "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
        manifest_path.display()
    );
}
