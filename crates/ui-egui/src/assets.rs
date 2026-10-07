use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use std::collections::BTreeMap;

pub const LOGO_PNG: &[u8] = include_bytes!("../../../assets/craftlauncher-logo.png");

#[derive(Default)]
pub struct Assets {
    icons: BTreeMap<&'static str, TextureHandle>,
    heroes: BTreeMap<&'static str, TextureHandle>,
}
impl Assets {
    pub fn load(ctx: &Context) -> Self {
        let mut result = Self::default();
        if let Some(texture) = load(ctx, "launcher", LOGO_PNG) {
            result.icons.insert("launcher", texture);
        }
        let items: [(&str, &[u8], &[u8]); 7] = [
            (
                "photocraft",
                include_bytes!("../../../assets/photocraft.webp"),
                include_bytes!("../../../assets/photocraft-hero.webp"),
            ),
            (
                "vectorcraft",
                include_bytes!("../../../assets/vectorcraft.webp"),
                include_bytes!("../../../assets/vectorcraft-hero.webp"),
            ),
            (
                "filmcraft",
                include_bytes!("../../../assets/filmcraft.webp"),
                include_bytes!("../../../assets/filmcraft-hero.webp"),
            ),
            (
                "lightcraft",
                include_bytes!("../../../assets/lightcraft.webp"),
                include_bytes!("../../../assets/lightcraft-hero.webp"),
            ),
            (
                "printcraft",
                include_bytes!("../../../assets/printcraft.webp"),
                include_bytes!("../../../assets/printcraft-hero.webp"),
            ),
            (
                "effectcraft",
                include_bytes!("../../../assets/effectcraft.webp"),
                include_bytes!("../../../assets/effectcraft-hero.webp"),
            ),
            (
                "designcraft",
                include_bytes!("../../../assets/designcraft.webp"),
                include_bytes!("../../../assets/designcraft-hero.webp"),
            ),
        ];
        for (id, icon, hero) in items {
            if let Some(texture) = load(ctx, id, icon) {
                result.icons.insert(id, texture);
            }
            if let Some(texture) = load(ctx, &format!("{id}-hero"), hero) {
                result.heroes.insert(id, texture);
            }
        }
        result
    }
    pub fn icon(&self, id: &str) -> Option<&TextureHandle> {
        self.icons.get(id)
    }
    pub fn hero(&self, id: &str) -> Option<&TextureHandle> {
        self.heroes.get(id)
    }
}
fn load(ctx: &Context, name: &str, bytes: &[u8]) -> Option<TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?.into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(ctx.load_texture(
        name,
        ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
        TextureOptions::LINEAR,
    ))
}
/// The bundled launcher identity, shared by native window and tray icons.
pub fn icon_rgba(size: usize) -> Vec<u8> {
    if size == 0 {
        return Vec::new();
    }
    let image = image::load_from_memory(LOGO_PNG)
        .expect("The bundled launcher logo must be a valid PNG")
        .into_rgba8();
    image::imageops::resize(
        &image,
        size as u32,
        size as u32,
        image::imageops::FilterType::Lanczos3,
    )
    .into_raw()
}

#[cfg(test)]
mod tests {
    use super::icon_rgba;

    #[test]
    fn bundled_logo_decodes_at_native_icon_sizes_and_retains_transparency() {
        for size in [24, 32, 64, 256] {
            let rgba = icon_rgba(size);
            assert_eq!(rgba.len(), size * size * 4);
            let (pixels, remainder) = rgba.as_chunks::<4>();
            assert!(remainder.is_empty());
            assert!(pixels.iter().any(|pixel| pixel[3] == 0));
            assert!(pixels.iter().any(|pixel| pixel[3] == 255));
        }
        assert!(icon_rgba(0).is_empty());
    }
}
