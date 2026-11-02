#[derive(Clone, Copy, Debug)]
pub struct App {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub discipline: &'static str,
    pub description: &'static str,
    pub detail: &'static str,
    pub color: [u8; 3],
    pub tags: [&'static str; 3],
    pub stage: &'static str,
}

pub const DISCIPLINES: [&str; 9] = [
    "AI & 3D",
    "Photography & illustration",
    "Video & animation",
    "Interface design",
    "CAD & drafting",
    "Audio production",
    "Office & productivity",
    "PDF & publishing",
    "Other Craft apps",
];

pub const CATALOG: [App; 15] = [
    App {
        id: "artcraft",
        discipline: "AI & 3D",
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
        discipline: "Photography & illustration",
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
        discipline: "Photography & illustration",
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
        discipline: "Video & animation",
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
        discipline: "Photography & illustration",
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
        discipline: "PDF & publishing",
        name: "PdfCraft",
        category: "PDF workspace",
        description: "Give your documents a little more craft.",
        detail: "Read, organize, combine, split and secure PDFs in one open-source document workbench.",
        color: [220, 100, 87],
        tags: ["PDF editing", "Documents", "Annotations"],
        stage: "Early alpha",
    },
    App {
        id: "effectcraft",
        discipline: "Video & animation",
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
        discipline: "PDF & publishing",
        name: "DesignCraft",
        category: "Layout & publishing",
        description: "Good ideas deserve great layouts.",
        detail: "An open-source page layout and publishing workspace for thoughtful typography and beautifully composed documents.",
        color: [208, 113, 160],
        tags: ["Page layout", "Publishing", "Typography"],
        stage: "In development",
    },
    App {
        id: "anicraft",
        discipline: "Video & animation",
        name: "AniCraft",
        category: "2D animation",
        description: "Draw and animate in a native 2D workspace.",
        detail: "An open-source 2D animation editor written from scratch in Rust.",
        color: [238, 156, 66],
        tags: ["2D animation", "Drawing", "Animation"],
        stage: "In development",
    },
    App {
        id: "cadcraft",
        discipline: "CAD & drafting",
        name: "CADCraft",
        category: "CAD & drafting",
        description: "Shape your next design.",
        detail: "Computer-aided design and drafting in a native Rust workspace.",
        color: [66, 160, 210],
        tags: ["CAD", "Drafting", "Design"],
        stage: "Early alpha",
    },
    App {
        id: "deckcraft",
        discipline: "Office & productivity",
        name: "DeckCraft",
        category: "Presentations",
        description: "Give your ideas a stage.",
        detail: "Create presentations and slide shows in a native Rust workspace.",
        color: [228, 140, 70],
        tags: ["Presentations", "Slides", "Documents"],
        stage: "Early alpha",
    },
    App {
        id: "figcraft",
        discipline: "Interface design",
        name: "FigCraft",
        category: "Interface design",
        description: "Design interfaces. Connect your ideas.",
        detail: "Interface design and prototyping in a native Rust workspace.",
        color: [172, 112, 218],
        tags: ["UI design", "Prototyping", "Design"],
        stage: "Early alpha",
    },
    App {
        id: "gridcraft",
        discipline: "Office & productivity",
        name: "GridCraft",
        category: "Spreadsheets",
        description: "Bring your numbers together.",
        detail: "An open-source spreadsheet workspace written in Rust.",
        color: [75, 167, 112],
        tags: ["Spreadsheets", "Data", "Documents"],
        stage: "Early alpha",
    },
    App {
        id: "soundcraft",
        discipline: "Audio production",
        name: "SoundCraft",
        category: "Audio production",
        description: "Find your sound.",
        detail: "An open-source audio production workspace written in Rust.",
        color: [90, 170, 170],
        tags: ["Audio", "Music", "Recording"],
        stage: "Early alpha",
    },
    App {
        id: "wordcraft",
        discipline: "Office & productivity",
        name: "WordCraft",
        category: "Word processing",
        description: "Make every word count.",
        detail: "An open-source word processing workspace written in Rust.",
        color: [80, 135, 220],
        tags: ["Writing", "Documents", "Word processing"],
        stage: "Early alpha",
    },
];

pub fn app(id: &str) -> crate::Result<App> {
    CATALOG
        .iter()
        .find(|app| app.id == id)
        .copied()
        .ok_or_else(|| crate::fail("Unknown app."))
}

impl App {
    /// Keep the persisted launcher ID stable across upstream repository renames.
    pub fn repository(self) -> &'static str {
        match self.id {
            "printcraft" => "pdfcraft",
            "anicraft" => "AniCraft",
            id => id,
        }
    }

    pub fn executable_name(self) -> &'static str {
        match self.id {
            "printcraft" => "pdfcraft",
            id => id,
        }
    }
}

/// Owned product metadata shared by the manager and its UI snapshots. Built-in
/// descriptions are presentation overrides; discovered products need no code.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Product {
    pub id: String,
    pub repository: String,
    pub name: String,
    pub category: String,
    pub discipline: String,
    pub description: String,
    pub detail: String,
    pub color: [u8; 3],
    pub tags: Vec<String>,
    pub stage: String,
    pub executable_name: String,
    pub github_id: Option<u64>,
    pub unavailable: bool,
    pub discovered: bool,
}

impl From<App> for Product {
    fn from(app: App) -> Self {
        Self {
            id: app.id.into(),
            repository: app.repository().into(),
            name: app.name.into(),
            category: app.category.into(),
            discipline: app.discipline.into(),
            description: app.description.into(),
            detail: app.detail.into(),
            color: app.color,
            tags: app.tags.into_iter().map(str::to_owned).collect(),
            stage: app.stage.into(),
            executable_name: app.executable_name().into(),
            github_id: None,
            unavailable: false,
            discovered: false,
        }
    }
}

/// Construct an owned catalog from built-ins and the last successful scan.
/// Reject unsafe identifiers and collisions before these values can become paths.
pub fn products(discovered: &[crate::workspace::DiscoveredApp]) -> Vec<Product> {
    let mut products: Vec<Product> = CATALOG.into_iter().map(Product::from).collect();
    for app in discovered {
        let id = if app.app_id.is_empty() {
            app.repository.to_ascii_lowercase()
        } else {
            app.app_id.clone()
        };
        let valid = |value: &str| {
            !value.is_empty()
                && value.len() <= 100
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        if !valid(&id)
            || !valid(&app.repository)
            || products.iter().any(|other| {
                other.id.eq_ignore_ascii_case(&id)
                    || other.repository.eq_ignore_ascii_case(&app.repository)
            })
        {
            continue;
        }
        products.push(Product {
            id: id.clone(),
            repository: app.repository.clone(),
            name: app.repository.clone(),
            category: "Craft app".into(),
            discipline: "Other Craft apps".into(),
            description: app.description.clone(),
            detail: app.description.clone(),
            color: [45, 129, 255],
            tags: vec!["Craft".into()],
            stage: if app.unavailable {
                "Unavailable"
            } else {
                "New"
            }
            .into(),
            executable_name: app.repository.to_ascii_lowercase(),
            github_id: app.github_id,
            unavailable: app.unavailable,
            discovered: true,
        });
    }
    products
}

#[cfg(test)]
mod product_tests {
    use super::*;
    use crate::workspace::DiscoveredApp;

    fn discovered(id: &str, repo: &str) -> DiscoveredApp {
        DiscoveredApp {
            app_id: id.into(),
            github_id: Some(42),
            unavailable: false,
            repository: repo.into(),
            description: "A new app".into(),
        }
    }

    #[test]
    fn owned_catalog_includes_discovery_and_rejects_path_and_builtin_collisions() {
        let products = products(&[
            discovered("newcraft", "NewCraft"),
            discovered("../escape", "SafeCraft"),
            discovered("photocraft", "OtherCraft"),
            discovered("othercraft", "pdfcraft"),
        ]);
        assert_eq!(products.len(), CATALOG.len() + 1);
        let new = products.iter().find(|p| p.id == "newcraft").unwrap();
        assert_eq!(new.repository, "NewCraft");
        assert_eq!(new.github_id, Some(42));
        assert!(new.discovered);
        let pdf = products.iter().find(|p| p.id == "printcraft").unwrap();
        assert_eq!(pdf.repository, "pdfcraft");
        assert!(!pdf.discovered);
    }
}
