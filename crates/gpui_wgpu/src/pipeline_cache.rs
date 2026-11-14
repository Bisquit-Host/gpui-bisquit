//! A Vulkan pipeline cache kept on disk, so shaders compile once per driver
//! rather than on every launch.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// Enables the disk cache; call before the first window opens.
pub fn set_pipeline_cache_dir(directory: PathBuf) {
    let _ = DIRECTORY.set(directory);
}

pub(crate) struct DiskPipelineCache {
    cache: wgpu::PipelineCache,
    path: PathBuf,
    dirty: AtomicBool,
}

impl DiskPipelineCache {
    pub(crate) fn open(adapter: &wgpu::Adapter, device: &wgpu::Device) -> Option<Arc<Self>> {
        if !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
            return None;
        }
        let path = DIRECTORY
            .get()?
            .join(wgpu::util::pipeline_cache_key(&adapter.get_info())?);
        let data = std::fs::read(&path).ok();
        // Safety: the file is only written from `get_data` under a key naming this
        // adapter and driver, and wgpu validates its header, falling back to an
        // empty cache on mismatch.
        let cache = unsafe {
            device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                label: Some("gpui_pipeline_cache"),
                data: data.as_deref(),
                fallback: true,
            })
        };
        Some(Arc::new(Self {
            cache,
            path,
            dirty: AtomicBool::new(false),
        }))
    }

    /// The cache to compile a new pipeline with; the next `save` writes it out.
    pub(crate) fn use_for_pipeline(&self) -> &wgpu::PipelineCache {
        self.dirty.store(true, Ordering::Relaxed);
        &self.cache
    }

    /// Writes the cache off-thread when pipelines were added since the last save.
    pub(crate) fn save(&self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let Some(data) = self.cache.get_data() else {
            return;
        };
        let path = self.path.clone();
        std::thread::spawn(move || {
            let temporary = path.with_extension("tmp");
            let result =
                std::fs::write(&temporary, data).and_then(|()| std::fs::rename(&temporary, &path));
            if let Err(error) = result {
                log::warn!("Could not save the pipeline cache: {error}");
            }
        });
    }
}
