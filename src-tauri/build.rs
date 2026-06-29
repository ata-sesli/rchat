use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    configure_macos_dev_swift_runtime_rpath();
    tauri_build::build()
}

fn configure_macos_dev_swift_runtime_rpath() {
    println!("cargo:rerun-if-env-changed=RCHAT_SWIFT_RUNTIME_DIR");
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");

    if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("macos") {
        return;
    }
    if env::var("PROFILE").ok().as_deref() != Some("debug") {
        return;
    }

    if let Some(runtime_dir) = find_swift_runtime_dir() {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", runtime_dir.display());
    } else {
        println!(
            "cargo:warning=Could not find libswift_Concurrency.dylib; \
             dev builds that load ScreenCaptureKit may need RCHAT_SWIFT_RUNTIME_DIR"
        );
    }
}

fn find_swift_runtime_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("RCHAT_SWIFT_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| contains_swift_concurrency(dir))
    {
        return Some(dir);
    }

    swift_runtime_candidates()
        .into_iter()
        .find(|dir| contains_swift_concurrency(dir))
}

fn swift_runtime_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(swift) = command_output("xcrun", &["--find", "swift"]) {
        if let Some(usr_dir) = Path::new(swift.trim()).parent().and_then(Path::parent) {
            push_swift_usr_candidates(&mut candidates, usr_dir);
        }
    }

    if let Some(developer_dir) = command_output("xcode-select", &["-p"]) {
        let developer_dir = PathBuf::from(developer_dir.trim());
        push_swift_usr_candidates(&mut candidates, &developer_dir.join("usr"));
        push_swift_usr_candidates(
            &mut candidates,
            &developer_dir.join("Toolchains/XcodeDefault.xctoolchain/usr"),
        );
    }

    push_swift_usr_candidates(
        &mut candidates,
        Path::new("/Library/Developer/CommandLineTools/usr"),
    );

    candidates
}

fn push_swift_usr_candidates(candidates: &mut Vec<PathBuf>, usr_dir: &Path) {
    candidates.push(usr_dir.join("lib/swift/macosx"));
    candidates.push(usr_dir.join("lib/swift-5.5/macosx"));
}

fn contains_swift_concurrency(dir: &Path) -> bool {
    dir.join("libswift_Concurrency.dylib").is_file()
}

fn command_output(command: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(command).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}
