//! Embeds the studio icon and version information into the Windows binary.
//!
//! The icon is generated from the logo by the `straw-icon` crate so the
//! repository never has to carry a binary blob that can drift from the art.

fn main() {
    println!("cargo:rerun-if-changed=assets/InnovLogo.png");

    // Build scripts are compiled for the host, so `cfg(target_os)` would be
    // true on a Windows build machine but false when cross-compiling from
    // Linux. The target being built is what decides whether a Windows
    // executable is being produced.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        embed_windows_resources();
    }
}

/// Adds the icon and the version metadata to the executable.
fn embed_windows_resources() {
    let logo = std::path::Path::new("assets/InnovLogo.png");
    if !logo.exists() {
        println!(
            "cargo:warning=assets/InnovLogo.png not found, skipping the icon"
        );
        return;
    }

    // Generate the multi-resolution icon next to the build output.
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // OUT_DIR can be relative when cargo runs from the package folder, and
    // winres resolves its paths against the working directory, so make it
    // absolute before using it.
    let out_dir = if out_dir.is_absolute() {
        out_dir
    } else {
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join(out_dir)
    };
    let ico_path = out_dir.join("icon.ico");
    let status = std::process::Command::new(std::env::var("CARGO").unwrap())
        .args(["run", "--quiet", "-p", "straw-icon", "--"])
        .arg(logo)
        .arg(&ico_path)
        .status();
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            println!("cargo:warning=straw-icon failed with {status}, skipping the icon");
            return;
        }
        Err(err) => {
            println!("cargo:warning=could not run straw-icon: {err}, skipping the icon");
            return;
        }
    }

    let mut res = winres::WindowsResource::new();
    // When cross-compiling from Linux the GNU toolchain lives under
    // target-prefixed names and winres otherwise looks for the MSVC SDK
    // directory, which does not exist here.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        // The mingw tools are named after the CPU and vendor, not after the
        // Rust target triple: x86_64-w64-mingw32-windres for
        // x86_64-pc-windows-gnu.
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
        let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
        // The Rust triple uses "pc" as the vendor (x86_64-pc-windows-gnu),
        // while the mingw tools are named x86_64-w64-mingw32-windres: the
        // environment part is "mingw32", not the triple's "gnu".
        let prefixes = [
            format!("{arch}-w64-mingw32"),
            format!("{arch}-w64-{env}"),
            format!("{arch}-pc-{env}"),
        ];
        for prefix in prefixes {
            if let Some(path) = find_tool(&format!("{prefix}-windres")) {
                res.set_windres_path(&path);
            }
            if let Some(path) = find_tool(&format!("{prefix}-ar")) {
                res.set_ar_path(&path);
            }
        }
        // winres runs the tools with this as the working directory, so it has
        // to be a real directory. It must be absolute: the paths winres passes
        // are relative to the manifest dir, and it would look for them inside
        // this folder.
        res.set_toolkit_path(&out_dir.to_string_lossy());
        // The resource archive holds nothing that Rust code references, so
        // the linker would drop it as unused. Linking it whole keeps the
        // icon and the version information in the executable.
        println!("cargo:rustc-link-arg-bins=-Wl,--whole-archive");
        println!("cargo:rustc-link-arg-bins=-lresource");
        println!("cargo:rustc-link-arg-bins=-Wl,--no-whole-archive");
    }
    // OUT_DIR can be relative when cargo is invoked from the package folder.
    // Every path handed to windres has to be absolute for the same reason.
    res.set_output_directory(&out_dir.to_string_lossy());
    res.set_icon(&ico_path.to_string_lossy());
    res.set("ProductName", "StrawPatcher");
    res.set("FileDescription", "StrawPatcher - Strawberry Panic! Spanish patcher");
    res.set("CompanyName", "innovStudio");
    res.set("LegalCopyright", "Fan translation project, not affiliated with the original publisher");
    res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    if let Err(err) = res.compile() {
        println!("cargo:warning=failed to embed the Windows resources: {err}");
    }
}

/// Locates a build tool, checking the PATH and the usual system locations.
///
/// The PATH seen by a build script is not always the same as the user's
/// shell, so the common binary directories are searched as well.
fn find_tool(command: &str) -> Option<String> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(command);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    for dir in ["/usr/bin", "/usr/local/bin", "/bin"] {
        let candidate = std::path::Path::new(dir).join(command);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}
