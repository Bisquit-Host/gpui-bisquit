use gpui::{PlatformHeadlessRenderer, Scene};
use gpui_wgpu::WgpuHeadlessRenderer;

#[test]
fn renderer_shaders_validate() -> anyhow::Result<()> {
    use gpui_wgpu::wgpu::naga;
    for source in [
        include_str!("../src/shaders_backdrop.wgsl"),
        concat!(
            include_str!("../src/shaders.wgsl"),
            include_str!("../src/shaders_storage.wgsl")
        ),
        concat!(
            include_str!("../src/shaders.wgsl"),
            include_str!("../src/shaders_webgl.wgsl")
        ),
    ] {
        let module = naga::front::wgsl::parse_str(source)?;
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)?;
    }
    Ok(())
}

/// The uniform struct must match `backdrop::Uniforms`, which asserts the same size.
#[test]
fn backdrop_uniforms_match_rust_layout() -> anyhow::Result<()> {
    use gpui_wgpu::wgpu::naga;
    let module = naga::front::wgsl::parse_str(include_str!("../src/shaders_backdrop.wgsl"))?;
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx())?;
    let (handle, _) = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("Backdrop"))
        .ok_or_else(|| anyhow::anyhow!("no Backdrop struct"))?;
    assert_eq!(layouter[handle].size, 304);
    Ok(())
}

fn quad(x: f32, width: f32, color: gpui::Hsla) -> gpui::Quad {
    let bounds = gpui::Bounds {
        origin: gpui::point(x.into(), 0.0.into()),
        size: gpui::size(width.into(), 32.0.into()),
    };
    gpui::Quad {
        order: 0,
        border_style: gpui::BorderStyle::Solid,
        bounds,
        content_mask: gpui::ContentMask { bounds },
        background: color.into(),
        border_color: color,
        corner_radii: gpui::Corners::default(),
        border_widths: gpui::Edges::default(),
    }
}

#[test]
fn batches_keep_equal_order_ties_and_ranges() {
    use gpui::PrimitiveBatch;
    let mut scene = Scene::default();
    for order in [0, 2, 2, 5, u32::MAX - 1] {
        let mut quad = quad(0., 32., gpui::red());
        quad.order = order;
        scene.quads.push(quad);
    }
    for order in [0, 1, 2, 6, u32::MAX - 1] {
        scene.backdrops.push(gpui::Backdrop {
            order,
            ..Default::default()
        });
    }
    scene.finish();
    let mut actual = Vec::new();
    for batch in scene.batches() {
        match batch {
            PrimitiveBatch::Quads(range) => {
                actual.extend(scene.quads[range].iter().map(|quad| (quad.order, 0)));
            }
            PrimitiveBatch::Backdrops(range) => {
                actual.extend(
                    scene.backdrops[range]
                        .iter()
                        .map(|backdrop| (backdrop.order, 1)),
                );
            }
            _ => panic!("Unexpected batch in quad/backdrop scene"),
        }
    }
    let mut expected: Vec<_> = scene
        .quads
        .iter()
        .map(|quad| (quad.order, 0))
        .chain(scene.backdrops.iter().map(|backdrop| (backdrop.order, 1)))
        .collect();
    expected.sort();
    assert_eq!(actual, expected);
    assert_eq!(Scene::default().batches().count(), 0);
}

fn glass(order: u32, y: f32, source: u8) -> gpui::Backdrop {
    let bounds = gpui::Bounds {
        origin: gpui::point(0.0.into(), y.into()),
        size: gpui::size(100.0.into(), 40.0.into()),
    };
    gpui::Backdrop {
        order,
        bounds,
        content_mask: gpui::ContentMask { bounds },
        kind: gpui::BackdropKind::Glass,
        effect: gpui::BackdropEffect {
            shape: bounds,
            source,
            ..Default::default()
        },
    }
}

fn capture(order: u32, y: f32, slot: u8) -> gpui::Backdrop {
    let mut backdrop = glass(order, y, 0);
    backdrop.kind = gpui::BackdropKind::Capture;
    backdrop.effect.slot = slot;
    backdrop
}

#[test]
fn backdrops_share_a_break_unless_their_source_changed() {
    let starts = |quads: &[(u32, f32)], backdrops: Vec<gpui::Backdrop>| {
        let mut scene = Scene::default();
        for &(order, y) in quads {
            let mut quad = quad(0., 100., gpui::red());
            quad.order = order;
            quad.bounds.origin.y = y.into();
            quad.content_mask.bounds = quad.bounds;
            scene.quads.push(quad);
        }
        scene.backdrops = backdrops;
        scene.finish();
        gpui_wgpu::backdrop_group_starts(&scene, 100, 400)
    };
    // Content drawn between two glasses elsewhere on screen leaves one break.
    assert_eq!(
        starts(&[(2, 300.)], vec![glass(1, 0., 0), glass(3, 100., 0)]),
        [0]
    );
    // Content over the second glass's source must be drawn before it is read.
    assert_eq!(
        starts(&[(2, 100.)], vec![glass(1, 0., 0), glass(3, 100., 0)]),
        [0, 1]
    );
    // A glass drawn over the next one's source counts as content too.
    assert_eq!(starts(&[], vec![glass(1, 0., 0), glass(2, 20., 0)]), [0, 1]);
    // A glass reading a slot does not read the target, so overdraw is fine.
    assert_eq!(
        starts(
            &[(3, 100.)],
            vec![capture(1, 100., 7), glass(2, 0., 0), glass(4, 100., 7)]
        ),
        [0]
    );
    // Recapturing a slot the group used needs a new break.
    assert_eq!(
        starts(
            &[],
            vec![capture(1, 0., 7), glass(2, 200., 7), capture(3, 300., 7)]
        ),
        [0, 2]
    );
}

#[test]
#[ignore = "Requires a headless wgpu adapter; run with --ignored on a GPU host"]
fn renderer_reuses_bindings_after_instance_counts_and_target_sizes_change() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    for width in [64, 32, 64] {
        for count in [1, 20, 1] {
            let mut scene = Scene::default();
            for _ in 0..count {
                scene.insert_primitive(quad(0., width as f32, gpui::red()));
            }
            scene.finish();
            let image = renderer.render_scene_to_image(
                &scene,
                gpui::size(gpui::DevicePixels(width), gpui::DevicePixels(32)),
            )?;
            assert_eq!(image.dimensions(), (width as u32, 32));
            assert_eq!(image.get_pixel(8, 8).0, [255, 0, 0, 255]);
        }
    }
    Ok(())
}
