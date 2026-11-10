//! Header-only image bounds. image 0.24's WebP constructor decodes before its
//! ImageDecoder limits can be set, so every embedded frame must be bounded first.
use super::*;
use crate::domain::model::AdminImageUploadPolicy;
use image::ImageDecoder;

fn invalid() -> flare_server_core::error::FlareError {
    flare_server_core::flare_err!(
        ErrorCode::InvalidParameter,
        "invalid or unsupported image dimensions"
    )
}
fn check((width, height): (u32, u32), policy: &AdminImageUploadPolicy) -> Result<(u32, u32)> {
    if width == 0
        || height == 0
        || width > policy.max_dimension
        || height > policy.max_dimension
        || u64::from(width) * u64::from(height) > policy.max_pixels
    {
        return Err(invalid());
    }
    Ok((width, height))
}
pub(super) fn dimensions(
    bytes: &[u8],
    format: image::ImageFormat,
    limits: image::io::Limits,
    policy: &AdminImageUploadPolicy,
) -> Result<(u32, u32)> {
    let cursor = std::io::Cursor::new(bytes);
    let dimensions = match format {
        image::ImageFormat::Png => image::codecs::png::PngDecoder::with_limits(cursor, limits)
            .map_err(|_| invalid())?
            .dimensions(),
        image::ImageFormat::Jpeg => image::codecs::jpeg::JpegDecoder::new(cursor)
            .map_err(|_| invalid())?
            .dimensions(),
        image::ImageFormat::Gif => return gif(bytes, policy),
        image::ImageFormat::WebP => return webp(bytes, policy),
        _ => return Err(invalid()),
    };
    check(dimensions, policy)
}
fn le24(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

fn gif(bytes: &[u8], policy: &AdminImageUploadPolicy) -> Result<(u32, u32)> {
    fn take<'a>(bytes: &mut &'a [u8], len: usize) -> Result<&'a [u8]> {
        let value = bytes.get(..len).ok_or_else(invalid)?;
        *bytes = bytes.get(len..).ok_or_else(invalid)?;
        Ok(value)
    }
    fn table(bytes: &mut &[u8], flags: u8) -> Result<()> {
        if flags & 0x80 != 0 {
            take(bytes, 3 * (1usize << ((flags & 7) + 1)))?;
        }
        Ok(())
    }
    fn blocks(bytes: &mut &[u8]) -> Result<()> {
        loop {
            let len = take(bytes, 1)?[0] as usize;
            if len == 0 {
                return Ok(());
            }
            take(bytes, len)?;
        }
    }
    let mut remaining = bytes;
    let header = take(&mut remaining, 13)?;
    let canvas = check(
        (
            u32::from(u16::from_le_bytes([header[6], header[7]])),
            u32::from(u16::from_le_bytes([header[8], header[9]])),
        ),
        policy,
    )?;
    table(&mut remaining, header[10])?;
    let canvas_pixels = u64::from(canvas.0) * u64::from(canvas.1);
    let mut decoded_pixels = 0u64;
    loop {
        match take(&mut remaining, 1)?[0] {
            0x21 => {
                take(&mut remaining, 1)?;
                blocks(&mut remaining)?;
            }
            0x2c => {
                let frame = take(&mut remaining, 9)?;
                let left = u32::from(u16::from_le_bytes([frame[0], frame[1]]));
                let top = u32::from(u16::from_le_bytes([frame[2], frame[3]]));
                let dimensions = check(
                    (
                        u32::from(u16::from_le_bytes([frame[4], frame[5]])),
                        u32::from(u16::from_le_bytes([frame[6], frame[7]])),
                    ),
                    policy,
                )?;
                if left + dimensions.0 > canvas.0 || top + dimensions.1 > canvas.1 {
                    return Err(invalid());
                }
                decoded_pixels = decoded_pixels
                    .checked_add(canvas_pixels)
                    .ok_or_else(invalid)?;
                if decoded_pixels > policy.max_pixels {
                    return Err(invalid());
                }
                table(&mut remaining, frame[8])?;
                if !(2..=8).contains(&take(&mut remaining, 1)?[0]) {
                    return Err(invalid());
                }
                blocks(&mut remaining)?;
            }
            0x3b => {
                return if decoded_pixels > 0 && remaining.is_empty() {
                    Ok(canvas)
                } else {
                    Err(invalid())
                };
            }
            _ => return Err(invalid()),
        }
    }
}
fn chunks(mut bytes: &[u8], mut visit: impl FnMut(&[u8], &[u8]) -> Result<()>) -> Result<()> {
    while !bytes.is_empty() {
        let header = bytes.get(..8).ok_or_else(invalid)?;
        let size = u32::from_le_bytes(header[4..8].try_into().map_err(|_| invalid())?) as usize;
        let end = 8usize.checked_add(size).ok_or_else(invalid)?;
        let padded = end.checked_add(size % 2).ok_or_else(invalid)?;
        let payload = bytes.get(8..end).ok_or_else(invalid)?;
        visit(&header[..4], payload)?;
        bytes = bytes.get(padded..).ok_or_else(invalid)?;
    }
    Ok(())
}
fn frame(
    kind: &[u8],
    payload: &[u8],
    policy: &AdminImageUploadPolicy,
) -> Result<Option<(u32, u32)>> {
    let dimensions = match kind {
        b"VP8 " => {
            let header = payload.get(..10).ok_or_else(invalid)?;
            if header[0] & 1 != 0 || header[3..6] != [0x9d, 0x01, 0x2a] {
                return Err(invalid());
            }
            (
                u32::from(u16::from_le_bytes([header[6], header[7]]) & 0x3fff),
                u32::from(u16::from_le_bytes([header[8], header[9]]) & 0x3fff),
            )
        }
        b"VP8L" => {
            let header = payload.get(..5).ok_or_else(invalid)?;
            if header[0] != 0x2f {
                return Err(invalid());
            }
            let bits = u32::from_le_bytes(header[1..5].try_into().map_err(|_| invalid())?);
            ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
        }
        _ => return Ok(None),
    };
    check(dimensions, policy).map(Some)
}
fn webp(bytes: &[u8], policy: &AdminImageUploadPolicy) -> Result<(u32, u32)> {
    let header = bytes.get(..12).ok_or_else(invalid)?;
    let size = u32::from_le_bytes(header[4..8].try_into().map_err(|_| invalid())?) as usize;
    if &header[..4] != b"RIFF"
        || &header[8..12] != b"WEBP"
        || size.checked_add(8) != Some(bytes.len())
    {
        return Err(invalid());
    }
    let mut canvas = None;
    let mut frames = 0;
    let mut decoded_pixels = 0u64;
    chunks(&bytes[12..], |kind, payload| {
        if kind == b"VP8X" {
            if canvas.is_some() || payload.len() != 10 {
                return Err(invalid());
            }
            canvas = Some(check(
                (le24(&payload[4..7]) + 1, le24(&payload[7..10]) + 1),
                policy,
            )?);
        } else if kind == b"ANMF" {
            let h = payload.get(..16).ok_or_else(invalid)?;
            let dimensions = check((le24(&h[6..9]) + 1, le24(&h[9..12]) + 1), policy)?;
            let (cw, ch) = canvas.ok_or_else(invalid)?;
            if u64::from(le24(&h[..3])) * 2 + u64::from(dimensions.0) > u64::from(cw)
                || u64::from(le24(&h[3..6])) * 2 + u64::from(dimensions.1) > u64::from(ch)
            {
                return Err(invalid());
            }
            let mut inner_frames = 0;
            chunks(&payload[16..], |inner_kind, inner| {
                if let Some(inner_dimensions) = frame(inner_kind, inner, policy)? {
                    if inner_dimensions != dimensions {
                        return Err(invalid());
                    }
                    inner_frames += 1;
                } else if inner_kind != b"ALPH" {
                    return Err(invalid());
                }
                Ok(())
            })?;
            if inner_frames != 1 {
                return Err(invalid());
            }
            decoded_pixels = decoded_pixels
                .checked_add(u64::from(cw) * u64::from(ch))
                .ok_or_else(invalid)?;
            frames += 1;
        } else if let Some(dimensions) = frame(kind, payload, policy)? {
            if canvas.is_some_and(|canvas| canvas != dimensions) {
                return Err(invalid());
            }
            canvas = Some(dimensions);
            frames += 1;
            decoded_pixels = decoded_pixels
                .checked_add(u64::from(dimensions.0) * u64::from(dimensions.1))
                .ok_or_else(invalid)?;
        }
        if decoded_pixels > policy.max_pixels {
            return Err(invalid());
        }
        Ok(())
    })?;
    if frames == 0 {
        return Err(invalid());
    }
    canvas.ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn webp_bounds_are_checked_before_decoder_allocation() {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend(18u32.to_le_bytes());
        bytes.extend(b"WEBPVP8L");
        bytes.extend(5u32.to_le_bytes());
        bytes.push(0x2f);
        bytes.extend((9999u32 | (9999u32 << 14)).to_le_bytes());
        bytes.push(0);
        assert!(webp(&bytes, &AdminImageUploadPolicy::default()).is_err());
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2, 3)
            .write_to(&mut encoded, image::ImageOutputFormat::WebP)
            .unwrap();
        assert_eq!(
            webp(&encoded.into_inner(), &AdminImageUploadPolicy::default()).unwrap(),
            (2, 3)
        );
    }

    #[test]
    fn animated_gif_has_a_cumulative_canvas_budget_before_frame_decode() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for _ in 0..2 {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::new(2, 2)))
                    .unwrap();
            }
        }
        assert_eq!(
            gif(&bytes, &AdminImageUploadPolicy::default()).unwrap(),
            (2, 2)
        );
        assert!(
            gif(
                &bytes,
                &AdminImageUploadPolicy {
                    max_pixels: 4,
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
