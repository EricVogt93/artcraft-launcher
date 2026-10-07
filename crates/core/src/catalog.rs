#[derive(Clone, Copy, Debug)]
pub struct App {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub detail: &'static str,
    pub color: [u8; 3],
    pub tags: [&'static str; 3],
    pub stage: &'static str,
}

pub const CATALOG: [App; 8] = [
    App {
        id: "artcraft",
        name: "ArtCraft",
        category: "AI studio",
        description: "Compose scenes. Bring your ideas to life.",
        detail: "An intentional crafting engine for artists, designers and filmmakers. Direct images and video with 3D composition and controllable AI.",
        color: [45, 129, 255],
        tags: ["AI generation", "3D compositing", "Image & video"],
        stage: "Early access",
    },
    App {
        id: "photocraft",
        name: "PhotoCraft",
        category: "Image editing",
        description: "Every pixel, exactly as you imagined it.",
        detail: "Layers, masks, brushes, adjustment layers and real PSD files. A familiar image editing workspace, rebuilt in Rust.",
        color: [52, 122, 236],
        tags: ["Photo editing", "Layers & masks", "PSD support"],
        stage: "Early alpha",
    },
    App {
        id: "vectorcraft",
        name: "VectorCraft",
        category: "Vector illustration",
        description: "From the first line to the finest detail.",
        detail: "Resolution-independent illustration with paths, shapes, typography and live appearance effects.",
        color: [240, 164, 58],
        tags: ["Vector drawing", "Typography", "Illustration"],
        stage: "In development",
    },
    App {
        id: "filmcraft",
        name: "FilmCraft",
        category: "Video editing",
        description: "Find your story. Make the cut.",
        detail: "A native video editing workspace with a multi-track timeline, color tools and video scopes.",
        color: [134, 98, 222],
        tags: ["Video editing", "Multi-track", "Color grading"],
        stage: "In development",
    },
    App {
        id: "lightcraft",
        name: "LightCraft",
        category: "Photography",
        description: "A new light on your photography.",
        detail: "Organize your photo library and develop raw photographs locally, with a non-destructive editing workflow.",
        color: [66, 165, 166],
        tags: ["RAW development", "Photo library", "Photography"],
        stage: "In development",
    },
    App {
        id: "printcraft",
        name: "PrintCraft",
        category: "PDF workspace",
        description: "Give your documents a little more craft.",
        detail: "Read, organize, combine, split and secure PDFs in one open-source document workbench.",
        color: [220, 100, 87],
        tags: ["PDF editing", "Documents", "Annotations"],
        stage: "Early alpha",
    },
    App {
        id: "effectcraft",
        name: "EffectCraft",
        category: "Motion & VFX",
        description: "Make the impossible move.",
        detail: "Create motion graphics and visual effects with a composition workspace, animated text and a layered timeline.",
        color: [169, 119, 207],
        tags: ["Motion graphics", "Compositing", "Animation"],
        stage: "In development",
    },
    App {
        id: "designcraft",
        name: "DesignCraft",
        category: "Layout & publishing",
        description: "Good ideas deserve great layouts.",
        detail: "An open-source page layout and publishing workspace for thoughtful typography and beautifully composed documents.",
        color: [208, 113, 160],
        tags: ["Page layout", "Publishing", "Typography"],
        stage: "In development",
    },
];

pub fn app(id: &str) -> crate::Result<App> {
    CATALOG
        .iter()
        .find(|app| app.id == id)
        .copied()
        .ok_or_else(|| crate::fail("Unknown app."))
}
