//! A tiny assembler for the 16-bit code the emulator puts in the guest's
//! memory (the shell's prompt, the secondary COMMAND.COM): bytes, and
//! jumps to labels resolved at the end, so the code can change without
//! offsets worked out by hand.

pub struct Asm {
    /// The offset the code is loaded at in its segment.
    origin: u16,
    code: Vec<u8>,
    labels: Vec<(&'static str, u16)>,
    /// (position of the operand, label, 1 for a short jump's displacement
    /// or 2 for an absolute word)
    fixups: Vec<(usize, &'static str, u8)>,
}

impl Asm {
    pub fn new(origin: u16) -> Self {
        Self { origin, code: Vec::new(), labels: Vec::new(), fixups: Vec::new() }
    }

    pub fn label(&mut self, name: &'static str) {
        self.labels.push((name, self.origin + self.code.len() as u16));
    }

    pub fn op(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }

    /// A short jump (`opcode` rel8) to a label.
    pub fn jump(&mut self, opcode: u8, target: &'static str) {
        self.code.push(opcode);
        self.fixups.push((self.code.len(), target, 1));
        self.code.push(0);
    }

    /// An instruction ending in the address of a label as a word.
    pub fn address(&mut self, bytes: &[u8], target: &'static str) {
        self.code.extend_from_slice(bytes);
        self.fixups.push((self.code.len(), target, 2));
        self.code.extend_from_slice(&[0, 0]);
    }

    /// Where a label is.
    pub fn at(&self, name: &str) -> u16 {
        self.labels.iter().find(|(n, _)| *n == name).map(|&(_, at)| at).unwrap_or_else(|| panic!("no label {}", name))
    }

    /// The code, with the jumps' and addresses' labels filled in.
    pub fn finish(mut self) -> Vec<u8> {
        for &(pos, target, size) in &self.fixups {
            let target = self.at(target);
            if size == 1 {
                let next = self.origin as i32 + pos as i32 + 1;
                let disp = i8::try_from(target as i32 - next).expect("short jump out of range");
                self.code[pos] = disp as u8;
            } else {
                self.code[pos..pos + 2].copy_from_slice(&target.to_le_bytes());
            }
        }
        self.code
    }
}
