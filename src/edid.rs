//! Minimal EDID parser: manufacturer, model name, serial and physical panel size.

#[derive(Clone, Debug)]
pub struct EdidInfo {
    /// Stable identifier of the panel, e.g. `GSM5C19-12345678`.
    pub id: String,
    /// Model name from the 0xFC descriptor, if present.
    pub name: Option<String>,
    /// Physical size of the image area in millimetres, in native (unrotated) orientation.
    pub size_mm: Option<(f32, f32)>,
}

const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

pub fn parse(e: &[u8]) -> Option<EdidInfo> {
    if e.len() < 128 || e[0..8] != HEADER {
        return None;
    }

    let m = u16::from_be_bytes([e[8], e[9]]);
    let letter = |v: u16| (b'A' - 1 + (v & 0x1F) as u8) as char;
    let mfg: String = [letter(m >> 10), letter(m >> 5), letter(m)].iter().collect();
    let product = u16::from_le_bytes([e[10], e[11]]);
    let serial = u32::from_le_bytes([e[12], e[13], e[14], e[15]]);

    let mut name = None;
    let mut serial_text = None;
    let mut dtd_mm = None;
    for k in 0..4 {
        let d = &e[54 + 18 * k..72 + 18 * k];
        if d[0] != 0 || d[1] != 0 {
            // Detailed timing descriptor: image size is given in millimetres.
            if dtd_mm.is_none() {
                let w = d[12] as u32 | ((d[14] as u32 & 0xF0) << 4);
                let h = d[13] as u32 | ((d[14] as u32 & 0x0F) << 8);
                if w > 0 && h > 0 {
                    dtd_mm = Some((w as f32, h as f32));
                }
            }
        } else if d[3] == 0xFC {
            name = Some(descriptor_text(&d[5..18]));
        } else if d[3] == 0xFF {
            serial_text = Some(descriptor_text(&d[5..18]));
        }
    }

    // Basic display parameters give the size in whole centimetres only.
    let cm = (e[21] as f32 * 10.0, e[22] as f32 * 10.0);
    let size_mm = match dtd_mm {
        // Some panels put an aspect ratio (16x9) or garbage here, so cross-check with the cm values.
        Some((w, h)) if w >= 100.0 && h >= 50.0 && (cm.0 == 0.0 || (w - cm.0).abs() <= 20.0) => {
            Some((w, h))
        }
        _ if cm.0 > 0.0 && cm.1 > 0.0 => Some(cm),
        _ => None,
    };

    let serial = match serial_text {
        Some(s) if !s.is_empty() => s,
        _ => format!("{serial:08X}"),
    };

    Some(EdidInfo {
        id: format!("{mfg}{product:04X}-{serial}"),
        name: name.filter(|n| !n.is_empty()),
        size_mm,
    })
}

fn descriptor_text(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0x0A && b != 0)
        .map(|&b| b as char)
        .collect::<String>()
        .trim()
        .to_string()
}
