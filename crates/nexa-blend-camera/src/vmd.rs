//! Camera-only VMD 0002 serialization. No model, light or physics records.
use std::io::{self, Write};
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraKey {
    pub frame:u32,
    pub distance:f32,
    pub center:[f32;3],
    /// VMD x/y/z Euler angles in radians.
    pub rotation:[f32;3],
    pub fov_deg:u32,
    pub perspective:bool,
}
pub fn write_camera_vmd<W:Write>(out:&mut W, keys:&[CameraKey])->io::Result<()> {
    if keys.len()>u32::MAX as usize || keys.windows(2).any(|v|v[0].frame>=v[1].frame)
        || keys.iter().any(|k|!k.distance.is_finite() || k.center.iter().chain(&k.rotation).any(|v|!v.is_finite())
            || !(1..=135).contains(&k.fov_deg)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput,"invalid VMD camera keys"));
    }
    let mut signature=[0u8;30];
    signature[..25].copy_from_slice(b"Vocaloid Motion Data 0002");
    out.write_all(&signature)?;
    out.write_all(&[0x83,0x4a,0x83,0x81,0x83,0x89,0x81,0x45,0x8f,0xc6,0x96,0xbe,0,0,0,0,0,0,0,0])?;
    out.write_all(&0u32.to_le_bytes())?;
    out.write_all(&0u32.to_le_bytes())?;
    out.write_all(&(keys.len() as u32).to_le_bytes())?;
    for key in keys {
        out.write_all(&key.frame.to_le_bytes())?;
        out.write_all(&key.distance.to_le_bytes())?;
        for value in key.center.into_iter().chain(key.rotation) {out.write_all(&value.to_le_bytes())?;}
        for _ in 0..6 {out.write_all(&[20,107,20,107])?;}
        out.write_all(&key.fov_deg.to_le_bytes())?;
        out.write_all(&[u8::from(!key.perspective)])?;
    }
    for _ in 0..3 {out.write_all(&0u32.to_le_bytes())?;}
    Ok(())
}
