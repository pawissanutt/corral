use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=CORRAL_BUILD_SHA");
    let reference = Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned());
    for name in std::iter::once("HEAD").chain(reference.as_deref()) {
        if let Ok(output) = Command::new("git")
            .args(["rev-parse", "--git-path", name])
            .output()
            && output.status.success()
        {
            println!(
                "cargo:rerun-if-changed={}",
                String::from_utf8_lossy(&output.stdout).trim()
            );
        }
    }
    let sha = std::env::var("CORRAL_BUILD_SHA").ok().or_else(|| {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    });
    if let Some(sha) = sha {
        println!("cargo:rustc-env=CORRAL_SOURCE_SHA={sha}");
    }
}
