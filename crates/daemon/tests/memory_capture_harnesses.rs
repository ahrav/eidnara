#[test]
fn memory_capture_harnesses_match_the_kernel_source_identity_set() {
    assert_eq!(
        memory_store::memory_capture::CAPTURE_HARNESSES,
        kernel::source_identity::HARNESSES,
        "memory_capture::CAPTURE_HARNESSES must mirror kernel::source_identity::HARNESSES"
    );
}
