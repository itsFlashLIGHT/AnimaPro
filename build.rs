use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=tests/support/fake_ffmpeg.rs");
    let target = env::var("TARGET").unwrap();
    let suffix = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let encoder =
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join(format!("test-encoder{suffix}"));
    let mut compiler = std::process::Command::new(env::var_os("RUSTC").unwrap());
    compiler
        .args(["--target", &target, "tests/support/fake_ffmpeg.rs", "-o"])
        .arg(&encoder);
    let linker_var = format!(
        "CARGO_TARGET_{}_LINKER",
        target.replace('-', "_").to_ascii_uppercase()
    );
    if let Some(linker) = env::var_os(linker_var) {
        compiler
            .arg("-C")
            .arg(format!("linker={}", PathBuf::from(linker).display()));
    }
    let status = compiler.status().expect("could not build test encoder");
    assert!(status.success(), "could not build test encoder");
    println!(
        "cargo:rustc-env=ANIMAPRO_TEST_ENCODER={}",
        encoder.display()
    );
    println!("cargo:rerun-if-changed=assets/AnimaPro.res");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let resource = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("assets")
            .join("AnimaPro.res");
        if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            println!("cargo:rustc-link-arg={}", resource.display());
        } else if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            let object = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("icon.obj");
            let status = std::process::Command::new("llvm-cvtres")
                .arg("/machine:x64")
                .arg(format!("/out:{}", object.display()))
                .arg(&resource)
                .status()
                .expect("llvm-cvtres is required for the Windows GNU icon");
            assert!(status.success(), "could not compile Windows icon");
            println!("cargo:rustc-link-arg={}", object.display());
        }
    }
}
