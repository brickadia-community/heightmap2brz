use std::path::PathBuf;

use crate::{
    map::{Colormap, ColormapPNG, Heightmap, HeightmapFlat, HeightmapPNG},
    util::{GenOptions, file_ext},
};

/// Copy the output file's absolute path to the OS clipboard as a file list so
/// it can be pasted directly into Brickadia.
pub fn copy_path_to_clipboard(out_file: &str) -> Result<(), String> {
    let mut full_path = std::path::Path::new(out_file)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(out_file))
        .to_string_lossy()
        .to_string();

    // lowercase the first letter
    full_path.get_mut(0..1).map(|s| s.make_ascii_lowercase());

    #[cfg(target_os = "windows")]
    {
        clipboard_win::raw::open().map_err(|e| format!("failed to open clipboard: {e}"))?;
        let set = clipboard_win::raw::set_file_list(&[full_path.clone()])
            .map_err(|e| format!("failed to set clipboard: {e}"));
        let close =
            clipboard_win::raw::close().map_err(|e| format!("failed to close clipboard: {e}"));
        set?;
        close?;
        log::info!("Wrote path {full_path} to clipboard");
    }

    #[cfg(not(target_os = "windows"))]
    {
        log::info!("Clipboard file path support is only available on Windows");
        log::info!("File saved to: {}", full_path);
    }

    Ok(())
}

/// Small square thumbnail for an image file path.
pub fn thumb(ui: &mut egui::Ui, image: &PathBuf) {
    ui.add(
        egui::Image::new(egui::ImageSource::Uri(std::borrow::Cow::from(format!(
            "file://{}",
            image.display().to_string().replace("\\", "/")
        ))))
        .fit_to_exact_size(egui::vec2(32.0, 32.0))
        .maintain_aspect_ratio(false),
    );
}

type MapPair = (Box<dyn Heightmap>, Box<dyn Colormap>);

pub fn maps_from_files(
    options: &GenOptions,
    heightmap_files: Vec<PathBuf>,
    colormap_file: Option<PathBuf>,
) -> Result<MapPair, String> {
    let heightmap_files: Vec<PathBuf> = heightmap_files.into_iter().collect();
    let first_heightmap = heightmap_files
        .first()
        .map(|s| s.to_owned())
        .unwrap_or_else(|| "".into());
    let colormap_file = colormap_file.unwrap_or(first_heightmap);

    // colormap file parsing
    let colormap = match file_ext(&colormap_file)
        .map(|s| s.to_lowercase())
        .as_deref()
    {
        Some("png") | Some("jpg") | Some("jpeg") => ColormapPNG::new(&colormap_file, options.lrgb)
            .map_err(|e| format!("Error reading colormap: {:?}", e))?,
        Some(ext) => {
            return Err(format!("Unsupported colormap format '{}'", ext));
        }
        None => {
            return Err(format!(
                "Missing colormap format for '{}'",
                colormap_file.display()
            ));
        }
    };

    // heightmap file parsing
    let heightmap: Box<dyn Heightmap> = if heightmap_files.iter().all(|f| {
        matches!(
            file_ext(f).map(|s| s.to_lowercase()).as_deref(),
            Some("png") | Some("jpg") | Some("jpeg")
        )
    }) {
        if options.img {
            Box::new(HeightmapFlat::new(colormap.size()).unwrap())
        } else {
            match HeightmapPNG::new(heightmap_files.iter().collect(), options.hdmap) {
                Ok(map) => Box::new(map),
                Err(error) => {
                    return Err(format!("Error reading heightmap: {:?}", error));
                }
            }
        }
    } else {
        return Err("Unsupported heightmap format".to_string());
    };

    Ok((heightmap, Box::new(colormap)))
}
