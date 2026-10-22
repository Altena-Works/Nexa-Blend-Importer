//! One deterministic PMX/VMD name map. Source names are display data, never paths.

use std::{collections::BTreeSet, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    Model,
    Root,
    Bone,
    Morph,
}

impl NameKind {
    fn label(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Root => "root",
            Self::Bone => "bone",
            Self::Morph => "morph",
        }
    }
}

pub struct NameInput<'a> {
    pub kind: NameKind,
    pub source_id: &'a str,
    pub original: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameAlias {
    pub kind: NameKind,
    pub source_id: String,
    pub original: String,
    pub alias: String,
    /// Strict CP932, nonempty, no NUL, at most 14 bytes (one VMD terminator).
    pub cp932: Vec<u8>,
}

impl NameAlias {
    pub fn vmd_field(&self) -> [u8; 15] {
        let mut field = [0; 15];
        field[..self.cp932.len()].copy_from_slice(&self.cp932);
        field
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameError {
    InvalidInput,
    Unencodable,
    InvalidCp932,
    EncodingFailed,
    UnsupportedPlatform,
    Allocation,
    TooManyNames,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "name must be nonempty and contain no NUL",
            Self::Unencodable => "name is not exactly representable in CP932",
            Self::InvalidCp932 => "invalid CP932 bytes",
            Self::EncodingFailed => "Windows CP932 conversion failed",
            Self::UnsupportedPlatform => "CP932 conversion requires Windows",
            Self::Allocation => "name allocation failed",
            Self::TooManyNames => "14-byte name namespace exhausted",
        })
    }
}
impl std::error::Error for NameError {}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn WideCharToMultiByte(
        code_page: u32,
        flags: u32,
        wide: *const u16,
        wide_len: i32,
        output: *mut u8,
        output_len: i32,
        default_char: *const u8,
        used_default: *mut i32,
    ) -> i32;
    fn MultiByteToWideChar(
        code_page: u32,
        flags: u32,
        bytes: *const u8,
        byte_len: i32,
        output: *mut u16,
        output_len: i32,
    ) -> i32;
}

/// Rejects best-fit conversions and any use of Windows' replacement character.
pub fn encode_cp932(value: &str) -> Result<Vec<u8>, NameError> {
    if value.is_empty() || value.contains('\0') {
        return Err(NameError::InvalidInput);
    }
    #[cfg(windows)]
    {
        let count = value.encode_utf16().count();
        let count = i32::try_from(count).map_err(|_| NameError::Allocation)?;
        let mut wide = Vec::new();
        wide.try_reserve_exact(count as usize)
            .map_err(|_| NameError::Allocation)?;
        wide.extend(value.encode_utf16());
        let mut replaced = 0;
        let size = unsafe {
            WideCharToMultiByte(
                932,
                0x400,
                wide.as_ptr(),
                count,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                &mut replaced,
            )
        };
        if replaced != 0 {
            return Err(NameError::Unencodable);
        }
        if size <= 0 {
            return Err(NameError::EncodingFailed);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size as usize)
            .map_err(|_| NameError::Allocation)?;
        bytes.resize(size as usize, 0);
        let written = unsafe {
            WideCharToMultiByte(
                932,
                0x400,
                wide.as_ptr(),
                count,
                bytes.as_mut_ptr(),
                size,
                std::ptr::null(),
                &mut replaced,
            )
        };
        if replaced != 0 {
            return Err(NameError::Unencodable);
        }
        if written != size {
            return Err(NameError::EncodingFailed);
        }
        Ok(bytes)
    }
    #[cfg(not(windows))]
    {
        if value.is_ascii() {
            Ok(value.as_bytes().to_vec())
        } else {
            Err(NameError::UnsupportedPlatform)
        }
    }
}

pub fn decode_cp932(bytes: &[u8]) -> Result<String, NameError> {
    if bytes.is_empty() || bytes.contains(&0) {
        return Err(NameError::InvalidCp932);
    }
    #[cfg(windows)]
    {
        let length = i32::try_from(bytes.len()).map_err(|_| NameError::InvalidCp932)?;
        let count =
            unsafe { MultiByteToWideChar(932, 8, bytes.as_ptr(), length, std::ptr::null_mut(), 0) };
        if count <= 0 {
            return Err(NameError::InvalidCp932);
        }
        let mut wide = Vec::new();
        wide.try_reserve_exact(count as usize)
            .map_err(|_| NameError::Allocation)?;
        wide.resize(count as usize, 0);
        let written = unsafe {
            MultiByteToWideChar(932, 8, bytes.as_ptr(), length, wide.as_mut_ptr(), count)
        };
        if written != count {
            return Err(NameError::InvalidCp932);
        }
        String::from_utf16(&wide).map_err(|_| NameError::InvalidCp932)
    }
    #[cfg(not(windows))]
    {
        if bytes.is_ascii() {
            String::from_utf8(bytes.to_vec()).map_err(|_| NameError::InvalidCp932)
        } else {
            Err(NameError::UnsupportedPlatform)
        }
    }
}

/// Retains unique short originals. Other names receive generated ASCII aliases;
/// no multibyte string is truncated. Natural aliases are reserved before generation.
pub fn build_name_map(inputs: &[NameInput<'_>]) -> Result<Vec<NameAlias>, NameError> {
    let mut candidates = Vec::new();
    candidates
        .try_reserve_exact(inputs.len())
        .map_err(|_| NameError::Allocation)?;
    let mut reserved = BTreeSet::new();
    for input in inputs {
        if input.source_id.is_empty() || input.source_id.contains('\0') {
            return Err(NameError::InvalidInput);
        }
        let bytes = match encode_cp932(input.original) {
            Ok(bytes) if bytes.len() <= 14 && !bytes.iter().any(u8::is_ascii_control) => {
                Some(bytes)
            }
            Ok(_) | Err(NameError::Unencodable) => None,
            Err(error) => return Err(error),
        };
        if let Some(bytes) = &bytes {
            reserved.insert(bytes.clone());
        }
        candidates.push(bytes);
    }
    let mut used = BTreeSet::new();
    let mut output = Vec::new();
    output
        .try_reserve_exact(inputs.len())
        .map_err(|_| NameError::Allocation)?;
    let mut serial = 1_u64;
    for (input, candidate) in inputs.iter().zip(candidates) {
        let (alias, cp932) = if let Some(bytes) = candidate.filter(|bytes| !used.contains(bytes)) {
            (input.original.to_owned(), bytes)
        } else {
            loop {
                let alias = format!("{}_{}", input.kind.label(), serial);
                serial = serial.checked_add(1).ok_or(NameError::TooManyNames)?;
                if alias.len() > 14 {
                    return Err(NameError::TooManyNames);
                }
                let bytes = alias.as_bytes().to_vec();
                if !reserved.contains(&bytes) && !used.contains(&bytes) {
                    break (alias, bytes);
                }
            }
        };
        used.insert(cp932.clone());
        output.push(NameAlias {
            kind: input.kind,
            source_id: input.source_id.to_owned(),
            original: input.original.to_owned(),
            alias,
            cp932,
        });
    }
    Ok(output)
}

/// UTF-8 TSV; backslash, tab and line endings in source fields are escaped.
pub fn mapping_tsv<'a>(aliases: impl IntoIterator<Item = &'a NameAlias>) -> String {
    fn escaped(value: &str) -> String {
        value
            .replace('\\', "\\\\")
            .replace('\t', "\\t")
            .replace('\r', "\\r")
            .replace('\n', "\\n")
    }
    let mut text = String::from("kind\tsource_id\toriginal\tpmx_vmd_alias\tcp932_hex\n");
    for name in aliases {
        let hex: String = name
            .cp932
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect();
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            name.kind.label(),
            escaped(&name.source_id),
            escaped(&name.original),
            escaped(&name.alias),
            hex
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn japanese_long_unencodable_and_collision_names_share_exact_aliases() {
        assert_eq!(encode_cp932("日本").unwrap(), [0x93, 0xFA, 0x96, 0x7B]);
        assert_eq!(decode_cp932(&[0x93, 0xFA, 0x96, 0x7B]).unwrap(), "日本");
        assert_eq!(encode_cp932("😀"), Err(NameError::Unencodable));
        assert!(decode_cp932(&[0x81]).is_err());
        let long = "日本日本日本日本日本";
        let inputs = [
            NameInput {
                kind: NameKind::Root,
                source_id: "root",
                original: "root",
            },
            NameInput {
                kind: NameKind::Bone,
                source_id: "b1",
                original: "日本",
            },
            NameInput {
                kind: NameKind::Bone,
                source_id: "b2",
                original: long,
            },
            NameInput {
                kind: NameKind::Morph,
                source_id: "m1",
                original: "😀",
            },
            NameInput {
                kind: NameKind::Bone,
                source_id: "b3",
                original: "日本",
            },
            NameInput {
                kind: NameKind::Bone,
                source_id: "b4",
                original: "bone_1",
            },
            NameInput {
                kind: NameKind::Bone,
                source_id: "b5",
                original: "123456789012345",
            },
            NameInput {
                kind: NameKind::Morph,
                source_id: "m2",
                original: "short\nname",
            },
        ];
        let names = build_name_map(&inputs).unwrap();
        assert_eq!(names[1].alias, "日本");
        assert_eq!(names[2].alias, "bone_2");
        assert_eq!(names[5].alias, "bone_1");
        assert_eq!(names[2].original, long);
        assert_eq!(
            names
                .iter()
                .map(|name| name.cp932.clone())
                .collect::<BTreeSet<_>>()
                .len(),
            names.len()
        );
        for name in &names {
            assert!(name.cp932.len() <= 14);
            assert_eq!(encode_cp932(&name.alias).unwrap(), name.cp932);
            assert_eq!(&name.vmd_field()[..name.cp932.len()], name.cp932);
            assert_eq!(name.vmd_field()[14], 0);
            assert!(!name.cp932.iter().any(u8::is_ascii_control));
        }
        let tsv = mapping_tsv(&names);
        assert!(tsv.contains(&format!("bone\tb2\t{long}\tbone_2\t")));
        assert_eq!(tsv.lines().count(), names.len() + 1);
    }
}
