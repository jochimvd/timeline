//! Bounded native WebP encoding and decoding shared by capture and exports.
use anyhow::{Result, ensure};

pub const MAX_ENCODED: usize = 64 * 1024 * 1024;
pub fn pixel_bytes(width: u32, height: u32) -> Result<usize> {
    ensure!(
        width > 0 && height > 0 && width <= 16383 && height <= 16383,
        "invalid image dimensions"
    );
    let bytes = u64::from(width) * u64::from(height) * 4;
    ensure!(
        bytes <= crate::config::RAW_PIXEL_BUDGET as u64,
        "image dimensions exceed pixel budget"
    );
    Ok(bytes as usize)
}
unsafe extern "C" {
    fn timeline_encode(
        pixels: *const u8,
        width: i32,
        height: i32,
        quality: f32,
        method: i32,
        output: *mut *mut u8,
    ) -> usize;
    fn timeline_encode_lossless(
        pixels: *const u8,
        width: i32,
        height: i32,
        output: *mut *mut u8,
    ) -> usize;
    fn WebPFree(pointer: *mut std::ffi::c_void);
    fn WebPGetInfo(data: *const u8, size: usize, width: *mut i32, height: *mut i32) -> i32;
    fn WebPDecodeBGRAInto(
        data: *const u8,
        size: usize,
        output: *mut u8,
        output_size: usize,
        stride: i32,
    ) -> *mut u8;
}
pub fn dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_ENCODED,
        "invalid encoded image size"
    );
    let (mut width, mut height) = (0, 0);
    ensure!(
        unsafe { WebPGetInfo(bytes.as_ptr(), bytes.len(), &mut width, &mut height) } != 0,
        "invalid WebP image"
    );
    pixel_bytes(width as u32, height as u32)?;
    Ok((width as u32, height as u32))
}
pub fn encode_bgra(pixels: &[u8], width: u32, height: u32, lossless: bool) -> Result<Vec<u8>> {
    ensure!(
        pixels.len() == pixel_bytes(width, height)?,
        "invalid BGRA pixel length"
    );
    let mut output = std::ptr::null_mut();
    let size = unsafe {
        if lossless {
            timeline_encode_lossless(pixels.as_ptr(), width as i32, height as i32, &mut output)
        } else {
            timeline_encode(
                pixels.as_ptr(),
                width as i32,
                height as i32,
                crate::config::WEBP_QUALITY,
                crate::config::WEBP_METHOD,
                &mut output,
            )
        }
    };
    struct Encoded(*mut u8);
    impl Drop for Encoded {
        fn drop(&mut self) {
            unsafe { WebPFree(self.0.cast()) };
        }
    }
    let allocation = Encoded(output);
    ensure!(
        size > 0 && size <= MAX_ENCODED,
        "WebP encoding failed or exceeded byte limit"
    );
    Ok(unsafe { std::slice::from_raw_parts(allocation.0, size) }.to_vec())
}
pub fn decode_bgra(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let (width, height) = dimensions(bytes)?;
    let mut pixels = vec![0; pixel_bytes(width, height)?];
    decode_into(bytes, &mut pixels, width, height, width * 4)?;
    Ok((width, height, pixels))
}
pub(crate) fn decode_into(
    bytes: &[u8],
    pixels: &mut [u8],
    width: u32,
    height: u32,
    stride: u32,
) -> Result<()> {
    ensure!(
        dimensions(bytes)? == (width, height),
        "tile dimensions mismatch"
    );
    ensure!(
        stride >= width * 4
            && pixels.len() as u64
                >= u64::from(height - 1) * u64::from(stride) + u64::from(width) * 4,
        "short pixel destination"
    );
    ensure!(
        !unsafe {
            WebPDecodeBGRAInto(
                bytes.as_ptr(),
                bytes.len(),
                pixels.as_mut_ptr(),
                pixels.len(),
                stride as i32,
            )
        }
        .is_null(),
        "corrupt WebP image"
    );
    Ok(())
}

/// At most 640 pixels on either edge; no upscaling and no intermediate export.
pub(crate) fn thumbnail(pixels: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    scaled(pixels, width, height, 640, 1024 * 1024)
}
/// Lossy rendition at most `max_edge` pixels on either edge, never upscaled.
pub(crate) fn scaled(
    pixels: &[u8],
    width: u32,
    height: u32,
    max_edge: u32,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    ensure!(
        max_edge > 0 && pixels.len() == pixel_bytes(width, height)?,
        "invalid thumbnail pixels"
    );
    let edge = width.max(height);
    let scale = edge.min(max_edge);
    let w = ((width * scale + edge / 2) / edge).max(1);
    let h = ((height * scale + edge / 2) / edge).max(1);
    unsafe extern "C" {
        fn timeline_thumbnail(
            pixels: *const u8,
            width: i32,
            height: i32,
            target_width: i32,
            target_height: i32,
            output: *mut *mut u8,
        ) -> usize;
    }
    let mut output = std::ptr::null_mut();
    let size = unsafe {
        timeline_thumbnail(
            pixels.as_ptr(),
            width as i32,
            height as i32,
            w as i32,
            h as i32,
            &mut output,
        )
    };
    let result = if size > 0 && size <= max_bytes && !output.is_null() {
        Ok(unsafe { std::slice::from_raw_parts(output, size) }.to_vec())
    } else {
        Err(anyhow::anyhow!("thumbnail encoding failed"))
    };
    unsafe { WebPFree(output.cast()) };
    result
}
