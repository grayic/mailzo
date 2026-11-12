use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(apple_ai)");
    apple_ai();
    tauri_build::build()
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Builds the Apple Intelligence piece (swift/AppleAI.swift) into the app, when this Mac's
/// Xcode is new enough to have Apple's FoundationModels framework. Without it the app still
/// builds; AI then just never uses the Mac's own model.
fn apple_ai() {
    println!("cargo:rerun-if-changed=swift/AppleAI.swift");
    println!("cargo:rerun-if-env-changed=MAILZO_NO_APPLE_AI");
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    // A new Xcode can bring the framework, so look again whenever it changes.
    if let Some(developer) = run("xcode-select", &["-p"]) {
        println!("cargo:rerun-if-changed={developer}/../Info.plist");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos")
        || std::env::var_os("MAILZO_NO_APPLE_AI").is_some()
    {
        return;
    }
    let Some(sdk) = run("xcrun", &["--sdk", "macosx", "--show-sdk-path"]) else {
        return;
    };
    let framework = Path::new(&sdk).join("System/Library/Frameworks/FoundationModels.framework");
    if !framework.join("Modules").exists() {
        println!("cargo:warning=Apple Intelligence left out: this Xcode is older than 26.");
        return;
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => return,
    };
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let library = Path::new(&out).join("libmailzo_apple_ai.a");
    let built = Command::new("xcrun")
        .args(["--sdk", "macosx", "swiftc"])
        .args(["-parse-as-library", "-emit-library", "-static", "-O"])
        .args(["-swift-version", "5", "-module-name", "MailzoAppleAI"])
        .args(["-target", &format!("{arch}-apple-macos12.0")])
        .args(["-sdk", &sdk])
        .arg("swift/AppleAI.swift")
        .arg("-o")
        .arg(&library)
        .status();
    if !built.is_ok_and(|s| s.success()) {
        println!("cargo:warning=Apple Intelligence left out: the Swift part didn't build.");
        return;
    }
    let swiftc = run("xcrun", &["--sdk", "macosx", "--find", "swiftc"]).unwrap_or_default();
    let toolchain_lib = Path::new(&swiftc)
        .parent()
        .and_then(|bin| bin.parent())
        .map(|usr| usr.join("lib/swift/macosx"));
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-lib=static=mailzo_apple_ai");
    if let Some(lib) = toolchain_lib {
        println!("cargo:rustc-link-search=native={}", lib.display());
    }
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    // Older Macs don't have the framework at all, so the app must still open there.
    println!("cargo:rustc-link-arg=-Wl,-weak_framework,FoundationModels");
    println!("cargo:rustc-cfg=apple_ai");
}
