//! Clipboard helpers used by the TUI's Ctrl-V image paste flow.
//!
//! Kept outside the terminal module so image encoding remains small and
//! independently testable.

/// Encode an RGBA pixel buffer as PNG bytes. `arboard` hands us raw
/// pixels; the daemon's upload route expects a real image file format
/// so agents can open it without further conversion.
///
/// Pure synchronous fn so unit tests can pin the magic-number prefix
/// without touching the OS clipboard. The error type is `String` —
/// preserved across the move from `terminal/app.rs` (TUI tests
/// pattern-match on the literal `"RGBA buffer size mismatch"`).
pub fn encode_rgba_as_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgba};
    // image 0.25 wants an owned `Vec<u8>` for `from_raw` (it stores
    // the container internally). The clone is unavoidable.
    let buf = ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "RGBA buffer size mismatch".to_string())?;
    let mut out = Vec::with_capacity(rgba.len() / 2);
    PngEncoder::new(&mut out)
        .write_image(buf.as_raw(), width, height, ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PNG file format magic bytes. The encoder is third-party
    /// (`image` 0.25), so the only thing worth asserting at our
    /// layer is that the output really is a PNG — anything else
    /// would be re-testing the encoder.
    const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

    #[test]
    fn encode_rgba_as_png_produces_png_magic_prefix() {
        // 2x2 image of opaque white. 4 bytes per pixel × 4 pixels =
        // 16 bytes. Tiny on purpose — keeps the test fast and the
        // failure surface narrow (any drift in the encoder's output
        // prefix is a wire-format regression we want to know about).
        let rgba = vec![0xff_u8; 16];
        let out = encode_rgba_as_png(2, 2, &rgba).expect("encode must succeed");
        assert!(
            out.len() >= PNG_MAGIC.len(),
            "output too small: {} bytes",
            out.len()
        );
        assert_eq!(&out[..PNG_MAGIC.len()], PNG_MAGIC, "missing PNG magic");
    }

    #[test]
    fn encode_rgba_as_png_rejects_size_mismatch() {
        // 2x2 should require 16 bytes; passing 15 must error rather
        // than panic. Defensive against arboard handing back a
        // truncated buffer mid-paste (unlikely but free to assert).
        let short = vec![0xff_u8; 15];
        let err = encode_rgba_as_png(2, 2, &short).unwrap_err();
        assert!(
            err.contains("size mismatch"),
            "expected size-mismatch error, got: {err}"
        );
    }
}
