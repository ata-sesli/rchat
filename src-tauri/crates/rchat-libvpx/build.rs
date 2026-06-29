fn main() {
    println!("cargo:rerun-if-changed=native/rchat_vpx.c");
    println!("cargo:rerun-if-changed=native/rchat_vpx.h");

    let vpx = pkg_config::Config::new()
        .atleast_version("1.8")
        .probe("vpx")
        .expect("system libvpx development package is required");

    cc::Build::new()
        .file("native/rchat_vpx.c")
        .include("native")
        .includes(vpx.include_paths)
        .warnings(true)
        .compile("rchat_vpx");
}
