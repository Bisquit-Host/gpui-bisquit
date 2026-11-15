//! The textures a frame's paths are rasterized into, held only while frames
//! draw paths.
//!
//! Upstream creates a frame-sized intermediate texture and its multisampled
//! twin for every frame, whatever it draws. Most windows never draw a path,
//! and on a 1080x2400 phone the pair is 52 MB. Here the first frame with a
//! path creates them and they are dropped once frames stop having any.

use gpui::{DevicePixels, Scene, Size};

use crate::wgpu_renderer::WgpuRendererCore;

/// Frames without a path after which the textures are dropped. Long enough
/// that a path which comes and goes (a chart behind a hover) keeps them.
const IDLE_FRAMES: u32 = 60;

/// The multisampled texture is cleared, drawn and resolved in one pass, so
/// tilers need no memory behind it.
pub(crate) const MSAA_USAGE: wgpu::TextureUsages =
    wgpu::TextureUsages::RENDER_ATTACHMENT.union(wgpu::TextureUsages::TRANSIENT_ATTACHMENT);

/// Called before a frame is recorded: makes the path textures exist at `size`
/// if `scene` draws paths into them.
pub(crate) fn prepare(renderer: &mut WgpuRendererCore, scene: &Scene, size: Size<DevicePixels>) {
    if draws_paths(scene) {
        renderer.fast_frame.path_idle_frames = 0;
        renderer.ensure_intermediate_textures(size);
        return;
    }
    if renderer.resources().path_intermediate_view.is_none() {
        return;
    }
    renderer.fast_frame.path_idle_frames += 1;
    if renderer.fast_frame.path_idle_frames >= IDLE_FRAMES {
        renderer.fast_frame.path_idle_frames = 0;
        renderer.resources_mut().invalidate_intermediate_textures();
    }
}

/// Whether `scene` or a rounded clip group in it has paths. Layer tiles
/// rasterize theirs into the tile cache's own textures.
fn draws_paths(scene: &Scene) -> bool {
    !scene.paths.is_empty()
        || scene.polychrome_sprites.iter().any(|sprite| {
            crate::fast::rounded_clip::group(scene, sprite.tile.texture_id)
                .and_then(|group| group.content.scene())
                .is_some_and(draws_paths)
        })
}

#[cfg(all(test, feature = "test-support", not(target_family = "wasm")))]
mod tests;
