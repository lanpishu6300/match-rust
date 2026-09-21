//! Linux-only build script.
//!
//! No bindgen / no clang: FFI is hand-written in `src/dpdk/ffi.rs` (layouts
//! verified against Ubuntu noble DPDK 23.11 headers). This script emits link
//! directives for the DPDK runtime libraries installed from the offline .deb
//! closure and compiles `src/dpdk/bridge.c` into a static lib. The bridge is
//! required because Ubuntu's DPDK 23.11 .so.24.0 does NOT export the
//! data-plane entry points (rte_eth_rx_burst / tx_burst / pktmbuf_alloc /
//! append / free are header inline functions) — the C shim compiled against
//! the DPDK headers reproduces them. On non-Linux the block is cfg'd away.

fn main() {
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rerun-if-changed=src/dpdk/ffi.rs");
        println!("cargo:rerun-if-changed=src/dpdk/mod.rs");
        println!("cargo:rerun-if-changed=src/dpdk/port.rs");
        println!("cargo:rerun-if-changed=src/dpdk/bridge.c");

        // Compile the C bridge against the DPDK headers shipped by
        // libdpdk-dev (installed into /usr/include in the image), or a
        // user-level install pointed at by $DPDK_HOME.
        let home = std::env::var("DPDK_HOME").ok();
        let mut lib_dirs = vec![
            "/usr/lib/aarch64-linux-gnu",
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib",
        ];
        if let Some(h) = &home {
            lib_dirs.insert(0, format!("{h}/lib/x86_64-linux-gnu"));
            lib_dirs.insert(0, format!("{h}/lib"));
        }

        let has_rte = lib_dirs.iter().any(|d| {
            std::fs::read_dir(d)
                .map(|rd| {
                    rd.flatten()
                        .any(|e| e.file_name().to_string_lossy().starts_with("librte_"))
                })
                .unwrap_or(false)
        });
        let has_headers = std::fs::metadata("/usr/include/dpdk/rte_ethdev.h").is_ok()
            || home
                .as_ref()
                .is_some_and(|h| std::fs::metadata(format!("{h}/include/dpdk/rte_ethdev.h")).is_ok());
        let has_cc = std::process::Command::new("cc")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !(has_rte && has_headers && has_cc) {
            println!(
                "cargo:warning=match-dpdk-io: DPDK runtime/headers/cc not all present \
                 (rte={has_rte} headers={has_headers} cc={has_cc}); building stub. \
                 Install libdpdk-dev + libpcap-dev + libnuma-dev (or use the offline \
                 Docker flow) to enable the real DPDK backend."
            );
            return;
        }

        println!("cargo:rustc-cfg=dpdk_available");

        // Compile the C bridge against the DPDK headers.
        let out = std::env::var("OUT_DIR").expect("OUT_DIR");
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
        let arch_inc = if std::fs::metadata("/usr/include/aarch64-linux-gnu/dpdk").is_ok() {
            "-I/usr/include/aarch64-linux-gnu/dpdk"
        } else if std::fs::metadata("/usr/include/x86_64-linux-gnu/dpdk").is_ok() {
            "-I/usr/include/x86_64-linux-gnu/dpdk"
        } else {
            home.as_ref()
                .map(|h| format!("-I{h}/include/dpdk"))
                .unwrap_or_default()
        };
        let main_inc = if std::fs::metadata("/usr/include/dpdk").is_ok() {
            "-I/usr/include/dpdk"
        } else {
            home.as_ref()
                .map(|h| format!("-I{h}/include/dpdk"))
                .unwrap_or_default()
        };
        let ok = std::process::Command::new(&cc)
            .args([
                "-c",
                "src/dpdk/bridge.c",
                "-o",
                &format!("{out}/bridge.o"),
                &main_inc,
                &arch_inc,
                "-include",
                "rte_config.h",
            ])
            .status()
            .expect("run cc")
            .success();
        assert!(ok, "gcc failed to compile src/dpdk/bridge.c");
        let ok = std::process::Command::new("ar")
            .args(["rcs", &format!("{out}/libbridge.a"), &format!("{out}/bridge.o")])
            .status()
            .expect("run ar")
            .success();
        assert!(ok, "ar failed for libbridge.a");
        println!("cargo:rustc-link-search=native={out}");
        println!("cargo:rustc-link-lib=static=bridge");

        // Link every installed librte_*.so (safe: the runtime closure is
        // exactly what the pcap PMD path needs) plus pcap/numa if present.
        for dir in ["/usr/lib/aarch64-linux-gnu", "/usr/lib/x86_64-linux-gnu", "/usr/lib"] {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if let Some(rest) = n.strip_prefix("librte_").and_then(|s| s.strip_suffix(".so")) {
                        println!("cargo:rustc-link-search=native={dir}");
                        println!("cargo:rustc-link-lib=dylib=rte_{rest}");
                        println!("cargo:rustc-link-arg=-Wl,--no-as-needed");
                    }
                }
            }
        }
        println!("cargo:rustc-link-lib=dylib=pcap");
        println!("cargo:rustc-link-lib=dylib=numa");
        println!("cargo:rustc-link-arg=-Wl,--no-as-needed");
    }
}
