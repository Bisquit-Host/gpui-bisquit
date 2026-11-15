//! The path intermediate texture and its MSAA texture exist only while frames
//! draw paths. They are drawable sized, tens of megabytes a window, and most
//! windows never draw a path, so a frame allocates them only when its scene
//! or a rounded clip group in it has paths, a size change drops them, and
//! they are released after a run of frames without paths.

use anyhow::Result;
use gpui::{DevicePixels, Scene, Size};

use crate::metal_renderer::MetalRenderer;

/// How many consecutive frames without paths a renderer keeps the textures for.
const FRAMES_WITHOUT_PATHS_BEFORE_RELEASE: u32 = 60;

#[derive(Default)]
pub(crate) struct PathTextures {
    frames_without_paths: u32,
}

/// Forwarded to by `MetalRenderer::render_frame` before it encodes `scene`.
pub(crate) fn prepare(
    renderer: &mut MetalRenderer,
    scene: &Scene,
    viewport: Size<DevicePixels>,
) -> Result<()> {
    if crate::fast::rounded_clip::has_paths(scene) {
        renderer.fast_path_textures.frames_without_paths = 0;
        return renderer.update_path_intermediate_textures(viewport);
    }
    let frames = &mut renderer.fast_path_textures.frames_without_paths;
    *frames = frames.saturating_add(1);
    if *frames >= FRAMES_WITHOUT_PATHS_BEFORE_RELEASE {
        release(renderer);
    }
    Ok(())
}

pub(crate) fn release(renderer: &mut MetalRenderer) {
    renderer.path_intermediate_texture = None;
    renderer.path_intermediate_msaa_texture = None;
}

#[cfg(test)]
mod tests {
    use anyhow::{Context as _, Result};
    use gpui::{
        Bounds, ContentMask, DevicePixels, Hsla, Path, Quad, ScaledPixels, Scene, Size, point, px,
        rgba, size,
    };
    use objc2_metal::MTLTexture as _;

    use super::FRAMES_WITHOUT_PATHS_BEFORE_RELEASE;
    use crate::metal_renderer::MetalRenderer;

    const VIEWPORT: Size<DevicePixels> = size(DevicePixels(96), DevicePixels(72));

    fn quad_scene() -> Scene {
        let mut scene = Scene::default();
        let bounds = Bounds {
            origin: point(ScaledPixels(4.), ScaledPixels(4.)),
            size: size(ScaledPixels(32.), ScaledPixels(32.)),
        };
        scene.insert_primitive(Quad {
            bounds,
            content_mask: ContentMask { bounds },
            background: Hsla::from(rgba(0xff0000ff)).into(),
            ..Default::default()
        });
        scene.finish();
        scene
    }

    fn path_scene() -> Scene {
        let mut scene = Scene::default();
        let mut path = Path::new(point(px(8.), px(8.)));
        path.line_to(point(px(60.), px(12.)));
        path.line_to(point(px(20.), px(50.)));
        path.line_to(point(px(8.), px(8.)));
        path.content_mask = ContentMask {
            bounds: Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(96.), px(72.)),
            },
        };
        path.color = Hsla::from(rgba(0x8800ffcc)).into();
        scene.insert_primitive(path.scale(1.));
        scene.finish();
        scene
    }

    fn path_texture_size(renderer: &MetalRenderer) -> Option<(usize, usize)> {
        let texture = renderer.path_intermediate_texture.as_deref()?;
        Some((texture.width(), texture.height()))
    }

    #[test]
    fn frames_without_paths_allocate_no_path_textures() -> Result<()> {
        let mut renderer = MetalRenderer::new_headless(Default::default())?;
        renderer.render_scene_to_image(&quad_scene(), VIEWPORT)?;
        renderer.render_scene(&quad_scene(), VIEWPORT)?;
        assert!(renderer.path_intermediate_texture.is_none());
        assert!(renderer.path_intermediate_msaa_texture.is_none());
        Ok(())
    }

    #[test]
    fn a_frame_with_paths_allocates_them_at_its_viewport_size() -> Result<()> {
        let mut renderer = MetalRenderer::new_headless(Default::default())?;
        renderer.render_scene_to_image(&path_scene(), VIEWPORT)?;
        assert_eq!(path_texture_size(&renderer), Some((96, 72)));
        if let Some(msaa) = &renderer.path_intermediate_msaa_texture {
            assert_eq!((msaa.width(), msaa.height()), (96, 72));
        }

        renderer.render_scene(&path_scene(), size(DevicePixels(64), DevicePixels(48)))?;
        assert_eq!(path_texture_size(&renderer), Some((64, 48)));
        Ok(())
    }

    #[test]
    fn path_textures_are_released_after_frames_without_paths() -> Result<()> {
        let mut renderer = MetalRenderer::new_headless(Default::default())?;
        renderer.render_scene(&path_scene(), VIEWPORT)?;
        let quads = quad_scene();
        for _ in 1..FRAMES_WITHOUT_PATHS_BEFORE_RELEASE {
            renderer.render_scene(&quads, VIEWPORT)?;
        }
        renderer
            .path_intermediate_texture
            .as_ref()
            .context("released before the last frame without paths")?;
        renderer.render_scene(&quads, VIEWPORT)?;
        assert!(renderer.path_intermediate_texture.is_none());
        assert!(renderer.path_intermediate_msaa_texture.is_none());
        Ok(())
    }

    #[test]
    fn a_resize_drops_path_textures() -> Result<()> {
        let mut renderer = MetalRenderer::new_headless(Default::default())?;
        renderer.render_scene(&path_scene(), VIEWPORT)?;
        assert!(renderer.path_intermediate_texture.is_some());
        renderer.update_drawable_size(size(DevicePixels(128), DevicePixels(96)));
        assert!(renderer.path_intermediate_texture.is_none());
        assert!(renderer.path_intermediate_msaa_texture.is_none());
        Ok(())
    }
}
