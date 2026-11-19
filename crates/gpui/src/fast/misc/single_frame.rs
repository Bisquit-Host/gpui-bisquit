//! A decoded still image as the one frame it is, without copying its pixels.

use image::Frame;
use smallvec::SmallVec;

/// The frames of a still image: `frame`, moved into place.
///
/// Upstream builds them with `SmallVec::from_elem(frame, 1)`, which clones
/// the frame into the vector and drops the one it was given, so every image
/// decoded is allocated and copied whole a second time: 64 MB more for a
/// 4096 x 4096 one.
pub(crate) fn of(frame: Frame) -> SmallVec<[Frame; 1]> {
    SmallVec::from_const([frame])
}

#[cfg(test)]
mod tests {
    use image::{Frame, RgbaImage};

    #[test]
    fn the_frame_keeps_its_pixels_where_they_were_decoded() {
        let frame = Frame::new(RgbaImage::new(4, 4));
        let pixels = frame.buffer().as_ptr();
        assert_eq!(super::of(frame)[0].buffer().as_ptr(), pixels);
    }
}
