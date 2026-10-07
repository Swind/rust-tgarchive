use tgarchive::infrastructure::media::{checked_media_size, detect_image_type};

#[test]
fn image_signatures_determine_content_type() {
    assert_eq!(
        detect_image_type(b"\xff\xd8\xff\xe0"),
        Some(("image/jpeg", "jpg"))
    );
    assert_eq!(
        detect_image_type(b"\x89PNG\r\n\x1a\nrest"),
        Some(("image/png", "png"))
    );
    assert_eq!(
        detect_image_type(b"RIFFxxxxWEBP"),
        Some(("image/webp", "webp"))
    );
    assert_eq!(detect_image_type(b"not an image"), None);
    assert_eq!(detect_image_type(b"RIFFxxxxNOPE"), None);
}

#[test]
fn streamed_size_cap_applies_to_unknown_and_known_sizes() {
    let cap = 20 * 1024 * 1024;
    assert_eq!(checked_media_size(0, cap), Some(cap));
    assert_eq!(checked_media_size(cap - 1, 1), Some(cap));
    assert_eq!(checked_media_size(cap, 1), None);
    assert_eq!(checked_media_size(usize::MAX, 1), None);
}
