//! Vertical stretch of painted content, for Android's stretch overscroll.
//!
//! [`Window::with_vertical_stretch`] scales everything painted inside it about
//! a horizontal line. The scene stores primitives already stretched, so its
//! replays, rounded clips and layers draw them as painted; a retained subtree
//! is reused only under the stretch it was painted with.

use crate::{
    point, size, Bounds, ContentMask, Pixels, Primitive, ScaledPixels, TransformationMatrix, Window,
};

/// `y ↦ y · scale + offset` in scaled pixels, clipped to `clip`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stretch {
    scale: f32,
    offset: f32,
    clip: Bounds<ScaledPixels>,
}

impl Stretch {
    fn y(&self, y: ScaledPixels) -> ScaledPixels {
        ScaledPixels(y.0 * self.scale + self.offset)
    }

    fn bounds(&self, bounds: Bounds<ScaledPixels>) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(bounds.origin.x, self.y(bounds.origin.y)),
            size: size(
                bounds.size.width,
                ScaledPixels(bounds.size.height.0 * self.scale),
            ),
        }
    }

    fn mask(&self, mask: &mut ContentMask<ScaledPixels>) {
        mask.bounds = self.bounds(mask.bounds).intersect(&self.clip);
    }

    /// This stretch followed by `outer`.
    fn then(self, outer: Stretch) -> Stretch {
        Stretch {
            scale: self.scale * outer.scale,
            offset: self.offset * outer.scale + outer.offset,
            clip: outer.bounds(self.clip).intersect(&outer.clip),
        }
    }

    pub(crate) fn shape(&self, shape: &mut crate::fast::rounded_clip::Shape) {
        shape.bounds = self.bounds(shape.bounds);
        for radius in [
            &mut shape.radii.top_left,
            &mut shape.radii.top_right,
            &mut shape.radii.bottom_right,
            &mut shape.radii.bottom_left,
        ] {
            radius.height.0 *= self.scale;
        }
    }

    pub(crate) fn layer(&self, bounds: Bounds<ScaledPixels>) -> Bounds<ScaledPixels> {
        self.bounds(bounds)
    }

    /// A sprite's transformation maps window positions (`R·p + t`), so the
    /// stretch scales its second row. Unit transformations stay unit: the
    /// stretch moves the sprite's bounds instead.
    fn transformation(&self, matrix: &mut TransformationMatrix) {
        matrix.rotation_scale[1][0] *= self.scale;
        matrix.rotation_scale[1][1] *= self.scale;
        matrix.translation[1] = matrix.translation[1] * self.scale + self.offset;
    }

    pub(crate) fn apply(&self, primitive: &mut Primitive) {
        let stretch = |bounds: &mut Bounds<ScaledPixels>| *bounds = self.bounds(*bounds);
        match primitive {
            Primitive::Shadow(shadow) => {
                stretch(&mut shadow.bounds);
                stretch(&mut shadow.element_bounds);
                self.mask(&mut shadow.content_mask);
            }
            Primitive::Quad(quad) => {
                stretch(&mut quad.bounds);
                self.mask(&mut quad.content_mask);
            }
            Primitive::Path(path) => {
                stretch(&mut path.bounds);
                self.mask(&mut path.content_mask);
                for vertex in &mut path.vertices {
                    vertex.xy_position.y = self.y(vertex.xy_position.y);
                    // `Window::paint_path` leaves vertices' masks empty.
                    if !vertex.content_mask.bounds.is_empty() {
                        self.mask(&mut vertex.content_mask);
                    }
                }
            }
            Primitive::Underline(underline) => {
                stretch(&mut underline.bounds);
                self.mask(&mut underline.content_mask);
            }
            Primitive::MonochromeSprite(sprite) => {
                if sprite.transformation == TransformationMatrix::unit() {
                    stretch(&mut sprite.bounds);
                } else {
                    self.transformation(&mut sprite.transformation);
                }
                self.mask(&mut sprite.content_mask);
            }
            Primitive::SubpixelSprite(sprite) => {
                if sprite.transformation == TransformationMatrix::unit() {
                    stretch(&mut sprite.bounds);
                } else {
                    self.transformation(&mut sprite.transformation);
                }
                self.mask(&mut sprite.content_mask);
            }
            Primitive::PolychromeSprite(sprite) => {
                stretch(&mut sprite.bounds);
                self.mask(&mut sprite.content_mask);
            }
            Primitive::Surface(surface) => {
                stretch(&mut surface.bounds);
                self.mask(&mut surface.content_mask);
            }
            Primitive::Backdrop(backdrop) => {
                stretch(&mut backdrop.bounds);
                stretch(&mut backdrop.effect.shape);
                self.mask(&mut backdrop.content_mask);
            }
        }
    }
}

impl Window {
    /// Prepaints or paints `f` scaled vertically by `scale` about the line at
    /// `pivot` and clipped to `clip`, as Android's stretch overscroll draws a
    /// container. Hitboxes keep their unstretched bounds. Call it around both
    /// the prepaint and the paint of the same content.
    pub fn with_vertical_stretch<R>(
        &mut self,
        clip: Bounds<Pixels>,
        pivot: Pixels,
        scale: f32,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();
        if scale == 1. || !scale.is_finite() {
            return f(self);
        }
        let factor = self.scale_factor();
        let inner = Stretch {
            scale,
            offset: f32::from(pivot) * factor * (1. - scale),
            clip: clip.scale(factor),
        };
        let previous = self.next_frame.scene.stretch;
        self.next_frame.scene.stretch = Some(match previous {
            Some(outer) => inner.then(outer),
            None => inner,
        });
        let result = f(self);
        self.next_frame.scene.stretch = previous;
        result
    }
}
