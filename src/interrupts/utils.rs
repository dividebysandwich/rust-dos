use crate::bus::Bus;

/// Helper to read a string from memory (DS:DX) until 0x00 (ASCIIZ)
pub fn read_asciiz_string(bus: &Bus, addr: usize) -> String {
    let mut curr = addr;
    let mut chars = Vec::new();
    loop {
        let byte = bus.ram[curr];
        if byte == 0 {
            break;
        }
        chars.push(byte);
        curr += 1;
    }
    String::from_utf8_lossy(&chars).to_string()
}

/// Converts a filename pattern (e.g., "*.*", "FILE.TXT") to DOS FCB format (11 bytes).
pub fn pattern_to_fcb(pattern: &str) -> [u8; 11] {
    let mut fcb = [b' '; 11];
    let upper = pattern.to_uppercase();
    
    // Split into Name and Extension
    let (name, ext) = match upper.rsplit_once('.') {
        Some((n, e)) => (n, e),
        None => (upper.as_str(), ""),
    };

    // Process Name (first 8 bytes)
    for (i, byte) in name.bytes().enumerate() {
        if i >= 8 { break; }
        if byte == b'*' {
            // Fill remaining name chars with '?'
            for j in i..8 { fcb[j] = b'?'; }
            break;
        } else {
            fcb[i] = byte;
        }
    }

    // Process Extension (last 3 bytes)
    for (i, byte) in ext.bytes().enumerate() {
        if i >= 3 { break; }
        if byte == b'*' {
             // Fill remaining ext chars with '?'
            for j in i..3 { fcb[8 + j] = b'?'; }
            break;
        } else {
            fcb[8 + i] = byte;
        }
    }

    fcb
}

/// Helper: Reconstruct "NAME.EXT" from the DTA's fixed-width 11-byte template
pub fn read_dta_template(bus: &Bus, dta_phys: usize) -> String {
    let mut name = String::new();
    let mut ext = String::new();

    // Read Name (Offsets 1-8)
    for i in 0..8 {
        let c = bus.read_8(dta_phys + 1 + i);
        // DOS uses 0x20 (Space) for padding. 0x3F is '?'.
        if c > 0x20 { 
            name.push(c as char); 
        } else if c == b'?' {
            name.push('?');
        }
    }

    // Read Extension (Offsets 9-11)
    for i in 0..3 {
        let c = bus.read_8(dta_phys + 9 + i);
        if c > 0x20 { 
            ext.push(c as char); 
        } else if c == b'?' {
            ext.push('?');
        }
    }

    // Handle "????????.???" case (Equivalent to *.*)
    if name.chars().all(|c| c == '?') && ext.chars().all(|c| c == '?') {
        return "*.*".to_string();
    }

    if ext.is_empty() {
        name
    } else {
        format!("{}.{}", name, ext)
    }
}