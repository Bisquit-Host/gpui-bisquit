use gpui::{
    Bounds, ContentMask, DevicePixels, Path, PlatformHeadlessRenderer as _, Quad, ScaledPixels,
    Scene, Size, point, px, size,
};

use crate::WgpuHeadlessRenderer;
use crate::fast::path_targets::IDLE_FRAMES;

const TARGET: Size<DevicePixels> = Size {
    width: DevicePixels(64),
    height: DevicePixels(48),
};

fn scene(with_path: bool) -> Scene {
    let mut scene = Scene::default();
    scene.insert_primitive(Quad {
        bounds: Bounds {
            origin: point(ScaledPixels(0.), ScaledPixels(0.)),
            size: size(ScaledPixels(64.), ScaledPixels(48.)),
        },
        content_mask: ContentMask {
            bounds: Bounds {
                origin: point(ScaledPixels(0.), ScaledPixels(0.)),
                size: size(ScaledPixels(64.), ScaledPixels(48.)),
            },
        },
        background: gpui::white().into(),
        ..Default::default()
    });
    if with_path {
        let mut path = Path::new(point(px(4.), px(4.)));
        path.line_to(point(px(28.), px(4.)));
        path.line_to(point(px(28.), px(20.)));
        path.line_to(point(px(4.), px(4.)));
        path.color = gpui::black().into();
        path.content_mask = ContentMask {
            bounds: Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(64.), px(48.)),
            },
        };
        scene.insert_primitive(path.scale(1.));
    }
    scene.finish();
    scene
}

fn path_texture_size(renderer: &WgpuHeadlessRenderer) -> Option<(u32, u32)> {
    let view = renderer.core.resources().path_intermediate_view.as_ref()?;
    Some((view.texture().width(), view.texture().height()))
}

#[test]
fn path_textures_exist_only_while_frames_draw_paths() {
    let Ok(mut renderer) = WgpuHeadlessRenderer::new() else {
        eprintln!("skipped: no wgpu adapter");
        return;
    };
    renderer
        .render_scene_to_image(&scene(false), TARGET)
        .unwrap();
    assert_eq!(path_texture_size(&renderer), None);

    renderer
        .render_scene_to_image(&scene(true), TARGET)
        .unwrap();
    assert_eq!(path_texture_size(&renderer), Some((64, 48)));

    for _ in 1..IDLE_FRAMES {
        renderer
            .render_scene_to_image(&scene(false), TARGET)
            .unwrap();
    }
    assert_eq!(path_texture_size(&renderer), Some((64, 48)));
    renderer
        .render_scene_to_image(&scene(false), TARGET)
        .unwrap();
    assert_eq!(path_texture_size(&renderer), None);

    renderer
        .render_scene_to_image(&scene(true), TARGET)
        .unwrap();
    assert_eq!(path_texture_size(&renderer), Some((64, 48)));
}
