fn main() {
    println!("cargo:rerun-if-env-changed=CORRAL_BUILD_SHA");
    if let Ok(sha) = std::env::var("CORRAL_BUILD_SHA") {
        println!("cargo:rustc-env=CORRAL_BUILD_SHA={sha}");
    }
}
