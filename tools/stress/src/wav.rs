//! Minimal WAV writer: mono, 32-bit float PCM.

use std::io::{self, Write};

pub fn write_mono_f32(out: &mut impl Write, sample_rate: u32, samples: &[f32]) -> io::Result<()> {
    let data_len = u32::try_from(samples.len() * 4)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "over 4 GiB"))?;
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data_len).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&3u16.to_le_bytes())?; // IEEE float
    out.write_all(&1u16.to_le_bytes())?; // mono
    out.write_all(&sample_rate.to_le_bytes())?;
    out.write_all(&(sample_rate * 4).to_le_bytes())?; // byte rate
    out.write_all(&4u16.to_le_bytes())?; // block align
    out.write_all(&32u16.to_le_bytes())?; // bits per sample
    out.write_all(b"data")?;
    out.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        out.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_data_sizes() {
        let mut buf = Vec::new();
        write_mono_f32(&mut buf, 48_000, &[0.5, -0.5, 0.0]).unwrap();
        assert_eq!(buf.len(), 44 + 12);
        assert_eq!(&buf[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(buf[4..8].try_into().unwrap()), 36 + 12);
        assert_eq!(&buf[36..40], b"data");
        assert_eq!(f32::from_le_bytes(buf[44..48].try_into().unwrap()), 0.5);
    }
}
