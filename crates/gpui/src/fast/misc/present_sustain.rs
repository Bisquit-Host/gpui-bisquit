//! Whether the last frame is presented again while fast input lasts.
//!
//! Upstream presents on every tick for a second after a burst of input, so a
//! variable refresh display does not lower its rate between frames. On
//! Android and iOS the rate does not follow presents: it comes from the
//! surface's frame rate vote and the display link. A present there draws the
//! whole unchanged scene again, so every touch gesture ended with up to a
//! second of duplicate frames.

pub(crate) const ENABLED: bool = !cfg!(any(target_os = "android", target_os = "ios"));
