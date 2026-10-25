pub(crate) fn push_u8(bytes: &mut Vec<u8>, value: u8) {
    bytes.push(value);
}

pub(crate) fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_fixed_string(bytes: &mut Vec<u8>, value: &[u8], len: usize) {
    let copied = value.len().min(len);
    bytes.extend_from_slice(&value[..copied]);
    bytes.resize(bytes.len() + len - copied, 0);
}

pub(crate) fn read_fixed_c_string(
    data: &[u8],
    offset: usize,
    len: usize,
) -> Result<String, String> {
    if offset + len > data.len() {
        return Err("unexpected EOF reading fixed string".to_string());
    }

    let raw = &data[offset..offset + len];
    Ok(read_lossy_c_string(raw))
}

pub(crate) fn read_lossy_c_string(raw: &[u8]) -> String {
    let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

pub(crate) fn read_c_string_at(data: &[u8], offset: usize) -> Result<(String, usize), String> {
    if offset > data.len() {
        return Err("unexpected EOF reading c string".to_string());
    }
    let relative_end = data[offset..]
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| "unterminated c string".to_string())?;
    let end = offset + relative_end;
    Ok((
        String::from_utf8_lossy(&data[offset..end]).into_owned(),
        end + 1,
    ))
}

pub(crate) fn read_u16(data: &[u8], offset: usize) -> Result<u16, String> {
    if offset + 2 > data.len() {
        return Err("unexpected EOF reading u16".to_string());
    }
    Ok(u16::from_le_bytes([data[offset], data[offset + 1]]))
}

pub(crate) fn read_optional_u16(data: &[u8], offset: usize) -> Result<Option<u16>, String> {
    if offset >= data.len() {
        return Ok(None);
    }
    read_u16(data, offset).map(Some)
}

pub(crate) fn read_i16(data: &[u8], offset: usize) -> Result<i16, String> {
    if offset + 2 > data.len() {
        return Err("unexpected EOF reading i16".to_string());
    }
    Ok(i16::from_le_bytes([data[offset], data[offset + 1]]))
}

pub(crate) fn read_optional_u32(data: &[u8], offset: usize) -> Result<Option<u32>, String> {
    if offset >= data.len() {
        return Ok(None);
    }
    read_u32(data, offset).map(Some)
}

pub(crate) fn read_u32(data: &[u8], offset: usize) -> Result<u32, String> {
    if offset + 4 > data.len() {
        return Err("unexpected EOF reading u32".to_string());
    }
    Ok(u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}

pub(crate) fn sign_extend_bits(value: u32, bits: u8) -> i32 {
    let shift = 32 - bits;
    ((value << shift) as i32) >> shift
}

pub(crate) fn read_i32(data: &[u8], offset: usize) -> Result<i32, String> {
    if offset + 4 > data.len() {
        return Err("unexpected EOF reading i32".to_string());
    }
    Ok(i32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}
