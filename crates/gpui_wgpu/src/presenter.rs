//! Presents frames on their own thread. On Android `queueBuffer` makes a
//! synchronous binder call to SurfaceFlinger, which would otherwise hold up
//! the render thread before it can start the next frame.

use std::sync::{Arc, Condvar, Mutex, mpsc};

pub(crate) struct Presenter {
    queue: wgpu::Queue,
    frames: mpsc::Sender<wgpu::SurfaceTexture>,
    pending: Arc<(Mutex<usize>, Condvar)>,
}

impl Presenter {
    pub(crate) fn new(queue: wgpu::Queue) -> Option<Self> {
        let (frames, receiver) = mpsc::channel::<wgpu::SurfaceTexture>();
        let pending = Arc::new((Mutex::new(0usize), Condvar::new()));
        let done = Arc::clone(&pending);
        let presenting = queue.clone();
        std::thread::Builder::new()
            .name("gpui-present".into())
            .spawn(move || {
                for frame in receiver {
                    presenting.present(frame);
                    let (count, presented) = &*done;
                    let mut count = count.lock().unwrap_or_else(|e| e.into_inner());
                    *count -= 1;
                    presented.notify_all();
                }
            })
            .ok()?;
        Some(Self {
            queue,
            frames,
            pending,
        })
    }

    pub(crate) fn present(&self, frame: wgpu::SurfaceTexture) {
        *self.pending.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        if let Err(mpsc::SendError(frame)) = self.frames.send(frame) {
            *self.pending.0.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
            self.queue.present(frame);
        }
    }

    /// Blocks until every queued frame is presented. The surface must not be
    /// acquired from, configured or released while a frame is in flight.
    pub(crate) fn settle(&self) {
        let (count, presented) = &*self.pending;
        let mut count = count.lock().unwrap_or_else(|e| e.into_inner());
        while *count > 0 {
            count = presented.wait(count).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        self.settle();
    }
}
