//! Hoard build script — compiles BPF C programs to eBPF bytecode.
//!
//! Requires `clang` and `libbpf-dev` for full functionality.
//! Builds without BPF if clang is not available.

use std::{io, path::Path};

fn main() -> io::Result<()> {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR must be set by cargo");
    let bpf_target = "src/bpf/hoard.bpf.c";
    let bpf_out = format!("{out_dir}/hoard.bpf.o");

    // Compile BPF C program to eBPF bytecode
    let cc = match which::which("clang") {
        Ok(c) => c,
        Err(_) => {
            eprintln!("⚠  clang not found — BPF programs will not be compiled");
            eprintln!("   Install: apt install clang llvm libbpf-dev");
            // Write a stub so the build doesn't fail
            std::fs::write(Path::new(&bpf_out), [])?;
            println!("cargo:rustc-env=HOARD_BPF_OBJECT={bpf_out}");
            println!("cargo:rerun-if-changed={bpf_target}");
            return Ok(());
        }
    };

    // Only need src/bpf/ for vmlinux.h — no system BPF headers
    let include_paths = ["src/bpf"];

    // Map Rust target_arch → BPF __TARGET_ARCH_* define
    let bpf_arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x86",
        Ok("aarch64") => "arm64",
        Ok("riscv64") => "riscv",
        Ok("powerpc64") | Ok("powerpc64le") => "powerpc",
        Ok("s390x") => "s390",
        other => {
            eprintln!("⚠  unknown target_arch={:?}, falling back to x86", other);
            "x86"
        }
    };

    let mut cmd = std::process::Command::new(cc);
    cmd.args([
        "-O2",
        "-g",
        "-target",
        "bpf",
        "-Wall",
        "-Werror",
        &format!("-D__TARGET_ARCH_{bpf_arch}"),
    ]);
    for inc in &include_paths {
        cmd.arg(format!("-I{inc}"));
    }
    // Use separate arg() to avoid type-uniformity issues in array literal
    cmd.args(["-c", bpf_target]);
    cmd.arg("-o").arg(Path::new(&bpf_out));

    let output = cmd.output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("⚠  BPF compilation failed (will continue without eBPF):");
        for line in stderr.lines().take(10) {
            eprintln!("   {}", line);
        }
        std::fs::write(Path::new(&bpf_out), [])?;
    }

    // Copy compiled BPF object to the standard runtime location.
    // The daemon looks for /usr/lib/hoard/hoard.bpf.o at startup.
    // We also embed the build-directory path for development convenience.
    let install_dest = "/usr/lib/hoard/hoard.bpf.o";
    if output.status.success()
        && std::fs::metadata(Path::new(&bpf_out))
            .map(|m| m.len())
            .unwrap_or(0)
            > 0
    {
        if let Some(parent) = std::path::Path::new(install_dest).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::copy(Path::new(&bpf_out), install_dest) {
            Ok(_) => eprintln!("✓ BPF object installed: {install_dest}"),
            Err(e) => eprintln!("⚠  Cannot install BPF object to {install_dest}: {e}"),
        }
    }

    // Embed the build-directory path for the runtime fallback
    println!("cargo:rustc-env=HOARD_BPF_OBJECT_BUILD={bpf_out}");
    println!("cargo:rerun-if-changed={bpf_target}");

    Ok(())
}
