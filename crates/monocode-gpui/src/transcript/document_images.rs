//! A markdown document opened from disk draws its local `![alt](path)` pictures. A chat never
//! does (the parser only produces `Block::Image` for a document), and a remote picture is never
//! loaded: the parser turns it into a link.
use super::*;
use gpui::AnyElement;
use std::path::{Path, PathBuf};

/// A picture's frame in a document. A picture's size is not known until it has loaded, and the
/// list measures a row before that, so the frame is fixed and the picture fits inside it.
const FRAME_HEIGHT: f32 = 320.;

/// Where `src` (as written in the document) lives: a relative path beside the document, `~` and
/// `file://` links as every file link reads them. `None` for anything that is not a local file.
pub(super) fn image_path(src: &str, folder: &str) -> Option<PathBuf> {
    let (written, _) = crate::file_editor::split_location(src);
    let path = monocode_core::files::document::resolve_path(Path::new(&written), Path::new(folder));
    (!monocode_core::files::remote::is_remote(&path)).then_some(path)
}

impl TranscriptView {
    pub(super) fn document_image(&self, ix: usize, src: &str, alt: &str) -> AnyElement {
        let label: SharedString = if alt.trim().is_empty() {
            src.to_owned().into()
        } else {
            alt.to_owned().into()
        };
        let Some(path) = image_path(src, self.file_link_base.as_deref().unwrap_or("")) else {
            return Self::missing_image(&label);
        };
        let missing = label.clone();
        div()
            .id(("document-image", ix))
            .debug_selector(move || format!("document-image-{ix}"))
            .w_full()
            .min_w_0()
            .h(px(FRAME_HEIGHT))
            .child(
                crate::image_memory::image(path)
                    .id(("document-image-file", ix))
                    .role(Role::Image)
                    .aria_label(label)
                    .size_full()
                    .object_fit(gpui::ObjectFit::Contain)
                    .with_fallback(move || Self::missing_image(&missing)),
            )
            .into_any_element()
    }

    fn missing_image(label: &SharedString) -> AnyElement {
        div()
            .text_size(px(13.))
            .text_color(content(0.5))
            .child(format!("{label} (image not found)"))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_found_beside_its_document() {
        assert_eq!(
            image_path("img/a.png", "/docs/guide"),
            Some(PathBuf::from("/docs/guide/img/a.png"))
        );
        assert_eq!(
            image_path("../shared/b.svg", "/docs/guide"),
            Some(PathBuf::from("/docs/shared/b.svg"))
        );
        assert_eq!(
            image_path("/abs/c.png", "/docs/guide"),
            Some(PathBuf::from("/abs/c.png"))
        );
        assert_eq!(
            image_path("my%20pic.png", "/docs"),
            Some(PathBuf::from("/docs/my pic.png"))
        );
        assert_eq!(
            image_path("file:///abs/d.png", "/docs"),
            Some(PathBuf::from("/abs/d.png"))
        );
    }
}
