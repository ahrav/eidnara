use host_runtime::local_embeddings::{LocalEmbeddingsComponent, LocalEmbeddingsLimits};

pub const BUNDLE_DIR: &str = "payload/model/gte-modernbert-base-f32";
pub const BUNDLE_MANIFEST: &str = "payload/model/gte-modernbert-base-f32/manifest.json";
pub const ORT_LIBRARY: &str = "payload/ort/libonnxruntime.so";

/// `sha256_of` answers a payload-relative path's recorded digest.
pub fn component<'a>(
    root: &std::path::Path,
    sha256_of: impl Fn(&str) -> Option<&'a str>,
) -> LocalEmbeddingsComponent {
    use host_runtime::local_embeddings::LocalEmbeddingsConfig;
    let (Some(ort), Some(bundle_manifest)) = (sha256_of(ORT_LIBRARY), sha256_of(BUNDLE_MANIFEST))
    else {
        return LocalEmbeddingsComponent::new(None);
    };
    LocalEmbeddingsComponent::new(Some(LocalEmbeddingsConfig {
        bundle_dir: root.join(BUNDLE_DIR),
        ort_library: root.join(ORT_LIBRARY),
        bundle_manifest_sha256: Some(bundle_manifest.to_owned()),
        ort_library_sha256: ort.to_owned(),
        limits: LocalEmbeddingsLimits::shipped_lane(),
    }))
}
