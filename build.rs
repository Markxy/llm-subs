//! Gives the Windows executable its icon. The window sets its own at runtime, but a pinned
//! taskbar shortcut and Explorer read the icon from inside the .exe, so without one they
//! show a blank page. It is drawn by the same code as the window's.

use std::path::{Path, PathBuf};

#[path = "src/ui/icon.rs"]
mod icon;

const ICON_SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/ui/icon.rs");
    // Not `cfg(windows)`: that would describe the machine building, not the one running.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let ico_path = out_dir.join("llm-subs.ico");
    std::fs::write(&ico_path, ico_file(&out_dir)?)?;
    let rc_path = out_dir.join("llm-subs.rc");
    std::fs::write(&rc_path, format!("1 ICON \"{}\"\n", ico_path.display().to_string().replace('\\', "/")))?;

    // Without a resource compiler the app still builds and works, only without this icon.
    if let Err(error) = embed_resource::compile(&rc_path, embed_resource::NONE).manifest_optional() {
        println!("cargo:warning=building without the executable icon: {error}");
    }
    Ok(())
}

/// An .ico file with every size as a PNG, which Windows has read since Vista.
fn ico_file(work_dir: &Path) -> anyhow::Result<Vec<u8>> {
    let mut images = Vec::new();
    for size in ICON_SIZES {
        let png_path = work_dir.join(format!("icon-{size}.png"));
        icon::write_png(&icon::render(size), &png_path)?;
        images.push((size, std::fs::read(&png_path)?));
    }

    const HEADER_SIZE: usize = 6;
    const ENTRY_SIZE: usize = 16;
    let mut offset = HEADER_SIZE + ENTRY_SIZE * images.len();
    let mut file = vec![0, 0, 1, 0];
    file.extend_from_slice(&(images.len() as u16).to_le_bytes());
    for (size, png) in &images {
        // One byte per dimension, where 0 stands for 256.
        let dimension = if *size >= 256 { 0 } else { *size as u8 };
        file.extend_from_slice(&[dimension, dimension, 0, 0]);
        file.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        file.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        file.extend_from_slice(&(png.len() as u32).to_le_bytes());
        file.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += png.len();
    }
    for (_, png) in &images {
        file.extend_from_slice(png);
    }
    Ok(file)
}
