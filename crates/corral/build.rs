fn main() {
    // The CLI/TUI dispatch frame needs the stack capacity used on Unix; Windows'
    // smaller executable default can overflow before even parsing --version.
    if let Ok(target) = std::env::var("TARGET") {
        if target.ends_with("-windows-msvc") {
            println!("cargo:rustc-link-arg-bin=corral=/STACK:8388608");
        } else if target.ends_with("-windows-gnu") {
            println!("cargo:rustc-link-arg-bin=corral=-Wl,--stack,8388608");
        }
    }
    println!("cargo:rerun-if-env-changed=CORRAL_BUILD_SHA");
    if let Ok(sha) = std::env::var("CORRAL_BUILD_SHA") {
        println!("cargo:rustc-env=CORRAL_BUILD_SHA={sha}");
    }
}
