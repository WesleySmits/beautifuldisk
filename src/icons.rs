//! Icons for cleanup items: the real macOS icon of an app or file when there
//! is one, else the tool's logo.

use std::path::Path;
use std::sync::Arc;

use gpui_kit::{Image, ImageFormat};
use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

/// Tool logos (Simple Icons, recolored to each brand).
#[derive(Clone, Copy)]
pub enum Logo {
    Android,
    Apple,
    Bun,
    Claude,
    CocoaPods,
    Git,
    Go,
    Gradle,
    HomeBrew,
    HuggingFace,
    Node,
    Npm,
    Ollama,
    OpenAi,
    Pnpm,
    Python,
    Rust,
    Uv,
    Yarn,
}

impl Logo {
    fn svg(self) -> &'static [u8] {
        macro_rules! brand {
            ($name:literal) => {
                include_bytes!(concat!("../assets/brand/", $name, ".svg"))
            };
        }
        match self {
            Logo::Android => brand!("android"),
            Logo::Apple => brand!("apple"),
            Logo::Bun => brand!("bun"),
            Logo::Claude => brand!("claude"),
            Logo::CocoaPods => brand!("cocoapods"),
            Logo::Git => brand!("git"),
            Logo::Go => brand!("go"),
            Logo::Gradle => brand!("gradle"),
            Logo::HomeBrew => brand!("homebrew"),
            Logo::HuggingFace => brand!("huggingface"),
            Logo::Node => brand!("nodedotjs"),
            Logo::Npm => brand!("npm"),
            Logo::Ollama => brand!("ollama"),
            Logo::OpenAi => brand!("openai"),
            Logo::Pnpm => brand!("pnpm"),
            Logo::Python => brand!("python"),
            Logo::Rust => brand!("rust"),
            Logo::Uv => brand!("uv"),
            Logo::Yarn => brand!("yarn"),
        }
    }
}

/// Where an item's icon comes from, best first.
#[derive(Clone, Copy)]
pub enum Source {
    /// An installed app by bundle ID, else the logo.
    App(&'static str, Option<Logo>),
    Logo(Logo),
    /// Whatever Finder shows for this file or folder.
    File,
}

pub fn load(source: Source, file: Option<&Path>) -> Option<Arc<Image>> {
    let png = match source {
        Source::App(bundle, logo) => app_icon(bundle).or_else(|| logo.map(svg)),
        Source::Logo(logo) => Some(svg(logo)),
        Source::File => file.and_then(|f| file_icon(&f.to_string_lossy())),
    };
    png.map(Arc::new)
}

fn svg(logo: Logo) -> Image {
    Image::from_bytes(ImageFormat::Svg, logo.svg().to_vec())
}

fn app_icon(bundle: &str) -> Option<Image> {
    let url = NSWorkspace::sharedWorkspace()
        .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle))?;
    file_icon(&url.path()?.to_string())
}

/// Finder's icon for `path`, rendered to a PNG big enough for a 2x screen.
fn file_icon(path: &str) -> Option<Image> {
    if !Path::new(path).exists() {
        return None;
    }
    let image = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(path));
    let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(64.0, 64.0));
    let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(Image::from_bytes(ImageFormat::Png, data.to_vec()))
}
