fn main() {
    println!("cargo:rerun-if-changed=native/rchat_vpx.c");
    println!("cargo:rerun-if-changed=native/rchat_vpx.h");

    // A Homebrew dylib install name is not portable to end-user Macs.
    // Require the archive on macOS instead of silently falling back to it.
    let static_vpx = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    let vpx = pkg_config::Config::new()
        .statik(static_vpx)
        .atleast_version("1.8")
        .probe("vpx")
        .expect("system libvpx development package is required");

    if static_vpx
        && !vpx
            .link_paths
            .iter()
            .any(|path| path.join("libvpx.a").is_file())
    {
        panic!("macOS release portability requires libvpx.a; install the static libvpx archive");
    }

    cc::Build::new()
        .file("native/rchat_vpx.c")
        .include("native")
        .includes(vpx.include_paths)
        .warnings(true)
        .compile("rchat_vpx");
}
