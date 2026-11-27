fn main() {
    #[cfg(any(target_os = "macos", windows))]
    {
        make_icon();
        println!("cargo:rerun-if-changed=../ui/dist");
        if std::path::Path::new("../ui/dist/index.html").is_file()
            && std::env::var_os("TAURI_CONFIG").is_none()
        {
            let config = r#"{"build":{"frontendDist":"../ui/dist"}}"#;
            std::env::set_var("TAURI_CONFIG", config);
            println!("cargo:rustc-env=TAURI_CONFIG={config}");
        }
        tauri_build::build();
    }
}

#[cfg(any(target_os = "macos", windows))]
fn make_icon() {
    use std::fs;
    use std::path::Path;
    let dir = Path::new("icons");
    fs::create_dir_all(dir).expect("icon directory");
    let mut image = vec![0; 32 * 32 * 4];
    let colors = [
        [248, 105, 117],
        [255, 173, 111],
        [252, 218, 139],
        [128, 216, 181],
        [107, 188, 244],
        [145, 145, 245],
        [204, 145, 243],
    ];
    for y in 2_i32..29 {
        for x in 2_i32..30 {
            let dx = x - 16;
            let dy = y - 16;
            let d = dx.abs() * 2 + dy.abs();
            if d < 14 {
                let rgb = if d < 10 {
                    [245, 247, 252]
                } else {
                    let sector = (((dy as f32).atan2(dx as f32) + std::f32::consts::PI)
                        / std::f32::consts::TAU
                        * 7.0)
                        .floor() as usize;
                    colors[sector.min(6)]
                };
                let index = (y as usize * 32 + x as usize) * 4;
                image[index..index + 3].copy_from_slice(&rgb);
                image[index + 3] = 255;
            }
        }
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 32, 32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("icon header")
            .write_image_data(&image)
            .expect("icon pixels");
    }
    fs::write(dir.join("icon.png"), &bytes).expect("PNG icon");
    let mut ico = vec![0, 0, 1, 0, 1, 0, 32, 32, 0, 0, 1, 0, 32, 0];
    ico.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22_u32.to_le_bytes());
    ico.extend_from_slice(&bytes);
    fs::write(dir.join("icon.ico"), ico).expect("ICO icon");
}
