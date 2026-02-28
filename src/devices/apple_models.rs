pub struct AppleModelInfo {
    pub name: &'static str,
    pub has_anc: bool,
    pub has_adaptive: bool,
    pub has_stem_controls: bool,
    pub has_conversation_awareness: bool,
}

pub const APPLE_VENDOR_ID: u16 = 0x004c;

pub fn model_info(product_id: u16) -> AppleModelInfo {
    match product_id {
        //                                                               ANC    Adaptive Stem   CA
        0x2002 => AppleModelInfo { name: "AirPods (1st gen)",   has_anc: false, has_adaptive: false, has_stem_controls: false, has_conversation_awareness: false },
        0x200f => AppleModelInfo { name: "AirPods (2nd gen)",   has_anc: false, has_adaptive: false, has_stem_controls: false, has_conversation_awareness: false },
        0x2013 => AppleModelInfo { name: "AirPods (3rd gen)",   has_anc: false, has_adaptive: false, has_stem_controls: true,  has_conversation_awareness: false },
        0x2019 => AppleModelInfo { name: "AirPods (4th gen)",   has_anc: false, has_adaptive: false, has_stem_controls: true,  has_conversation_awareness: false },
        0x201b => AppleModelInfo { name: "AirPods 4 ANC",       has_anc: true,  has_adaptive: true,  has_stem_controls: true,  has_conversation_awareness: true  },
        0x200e => AppleModelInfo { name: "AirPods Pro",          has_anc: true,  has_adaptive: false, has_stem_controls: true,  has_conversation_awareness: false },
        0x2014 => AppleModelInfo { name: "AirPods Pro 2",        has_anc: true,  has_adaptive: true,  has_stem_controls: true,  has_conversation_awareness: true  },
        0x2027 => AppleModelInfo { name: "AirPods Pro 3",        has_anc: true,  has_adaptive: true,  has_stem_controls: true,  has_conversation_awareness: true  },
        0x2024 => AppleModelInfo { name: "AirPods Pro (USB-C)",  has_anc: true,  has_adaptive: true,  has_stem_controls: true,  has_conversation_awareness: true  },
        0x200a => AppleModelInfo { name: "AirPods Max",          has_anc: true,  has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        0x201f => AppleModelInfo { name: "AirPods Max (2024)",   has_anc: true,  has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        0x200b => AppleModelInfo { name: "Powerbeats Pro",       has_anc: false, has_adaptive: false, has_stem_controls: false, has_conversation_awareness: false },
        0x201d => AppleModelInfo { name: "Powerbeats Pro 2",     has_anc: true,  has_adaptive: false, has_stem_controls: false, has_conversation_awareness: false },
        0x2006 => AppleModelInfo { name: "Beats Solo3",          has_anc: false, has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        0x200c => AppleModelInfo { name: "Beats Solo Pro",       has_anc: true,  has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        0x2009 => AppleModelInfo { name: "Beats Studio3",        has_anc: true,  has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        0x2005 => AppleModelInfo { name: "Beats X",              has_anc: false, has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        0x2010 => AppleModelInfo { name: "Beats Flex",           has_anc: false, has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        0x2003 => AppleModelInfo { name: "Powerbeats3",          has_anc: false, has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        0x200d => AppleModelInfo { name: "Powerbeats4",          has_anc: false, has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        0x2012 => AppleModelInfo { name: "Beats Fit Pro",        has_anc: true,  has_adaptive: false, has_stem_controls: false, has_conversation_awareness: false },
        0x2011 => AppleModelInfo { name: "Beats Studio Buds",    has_anc: true,  has_adaptive: false, has_stem_controls: false, has_conversation_awareness: false },
        0x2016 => AppleModelInfo { name: "Beats Studio Buds+",   has_anc: true,  has_adaptive: false, has_stem_controls: false, has_conversation_awareness: false },
        0x2017 => AppleModelInfo { name: "Beats Studio Pro",     has_anc: true,  has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        0x2025 => AppleModelInfo { name: "Beats Solo 4",          has_anc: true,  has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        0x2026 => AppleModelInfo { name: "Beats Solo Buds",      has_anc: false, has_adaptive: false,has_stem_controls: false, has_conversation_awareness: false },
        // Unknown Apple device — safe defaults
        _      => AppleModelInfo { name: "Apple Headphones",     has_anc: true,  has_adaptive: false, has_stem_controls: false, has_conversation_awareness: false },
    }
}

/// Returns true for models that require the AapInitExt packet to unlock Adaptive ANC.
pub fn needs_init_ext(product_id: u16) -> bool {
    matches!(product_id, 0x201b | 0x2014 | 0x2027 | 0x2024)
}

/// Parse a BlueZ Modalias string like "bluetooth:v004cp200edB087"
/// into (vendor_id, product_id).
pub fn parse_modalias(modalias: &str) -> Option<(u16, u16)> {
    let v_pos = modalias.find('v')?;
    let vendor = u16::from_str_radix(modalias.get(v_pos + 1..v_pos + 5)?, 16).ok()?;
    let p_pos = modalias.find('p')?;
    let product = u16::from_str_radix(modalias.get(p_pos + 1..p_pos + 5)?, 16).ok()?;
    Some((vendor, product))
}
