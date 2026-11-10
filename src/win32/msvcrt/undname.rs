//! The C++ name undecorator of `__unDName` and `type_info::name`: turns
//! a name Visual C++ decorated (`?name@type_info@@QBEPBDXZ`,
//! `.?AV?$basic_string@DU?$char_traits@D@std@@@std@@`'s type) back into
//! the declaration it stands for, with the spacing Microsoft's
//! undecorator gives it.
//!
//! A port of Wine's `dlls/msvcrt/undname.c`, which carries this notice:
//!
//! ```text
//! Demangle VC++ symbols into C function prototypes
//!
//! Copyright 2000 Jon Griffiths
//!           2004 Eric Pouech
//!
//! This library is free software; you can redistribute it and/or
//! modify it under the terms of the GNU Lesser General Public
//! License as published by the Free Software Foundation; either
//! version 2.1 of the License, or (at your option) any later version.
//!
//! This library is distributed in the hope that it will be useful,
//! but WITHOUT ANY WARRANTY; without even the implied warranty of
//! MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
//! Lesser General Public License for more details.
//!
//! You should have received a copy of the GNU Lesser General Public
//! License along with this library; if not, write to the Free Software
//! Foundation, Inc., 51 Franklin St, Fifth Floor, Boston, MA 02110-1301, USA
//! ```
//!
//! The parser keeps Wine's structure: three tables of strings for back
//! references (`names`, `args`) and the parts of a qualified name
//! (`stack`), each of which a reference may reach past its count as long
//! as an entry was once there (`max`), as Wine's arrays do. Where Wine's C
//! reads past the end of the name, this reads 0s.
//!
//! Wine's follows the undecorator of later Visual C++ versions. This one
//! gives what Visual C++ 6's MSVCRT.DLL gives (`undname_vc6.txt` has its
//! results, which the tests hold this to): two spaces before a variable's
//! name and after a type returned by value, a constructor's name in the
//! first back reference, and the name itself back for text past its end,
//! function templates, managed handles, vtordispex thunks, anonymous
//! namespaces and cointerfaces, which that version doesn't undecorate.
//! What it still does differently: names cut short, which Microsoft's
//! fills with " ?? ", the order of __ptr64, __unaligned and __restrict
//! together, and a second space after a const array's element type in an
//! argument.

/// The flags `__unDNameEx` takes (`UNDNAME_*` in dbghelp.h).
pub const NO_LEADING_UNDERSCORES: u32 = 0x0001;
pub const NO_MS_KEYWORDS: u32 = 0x0002;
pub const NO_FUNCTION_RETURNS: u32 = 0x0004;
pub const NO_ALLOCATION_LANGUAGE: u32 = 0x0010;
pub const NO_THISTYPE: u32 = 0x0060;
pub const NO_ACCESS_SPECIFIERS: u32 = 0x0080;
pub const NO_MEMBER_TYPE: u32 = 0x0200;
pub const NAME_ONLY: u32 = 0x1000;
pub const NO_ARGUMENTS: u32 = 0x2000;
/// Wine's own, which `NAME_ONLY` implies: no "class " before a class.
const NO_COMPLEX_TYPE: u32 = 0x8000;

/// The flags `type_info::name` passes: a type, in 32-bit code.
pub const TYPE_NAME: u32 = 0x2800;

/// A type's text either side of where a declarator's name would go:
/// `int (*` and `)(void)` for a pointer to a function.
#[derive(Clone, Debug, Default)]
struct DataType {
    left: Option<String>,
    right: Option<String>,
    flags: u32,
}

/// `DataType::flags`.
const DT_NO_LEADING_WS: u32 = 0x01;
const DT_NO_LRSEP_WS: u32 = 0x02;
/// A type with ?, which a function returns by value: Microsoft's puts
/// two spaces after it, "class C  __cdecl f(void)".
const DT_BY_VALUE: u32 = 0x04;
/// A pointer to a function or a member function: Microsoft's puts one
/// space before a variable's name after it, where others take two.
const DT_FUNCTION: u32 = 0x08;

/// `demangle_datatype`'s flags.
const IN_ARGS: u32 = 0x01;
const WS_AFTER_QUAL_IF: u32 = 0x02;

/// One of the parser's tables of strings.
#[derive(Clone, Debug, Default)]
struct Strings {
    /// The first entry a reference counts from.
    start: usize,
    /// The entries in use.
    num: usize,
    /// The most there have been, which references may reach.
    max: usize,
    elts: Vec<String>,
}

impl Strings {
    fn push(&mut self, s: &str) {
        if self.num < self.elts.len() {
            self.elts[self.num] = s.to_string();
        } else {
            self.elts.push(s.to_string());
        }
        self.num += 1;
        self.max = self.max.max(self.num);
    }

    fn get(&self, index: usize) -> Option<String> {
        let at = self.start + index;
        if at >= self.max {
            return None;
        }
        self.elts.get(at).cloned()
    }
}

/// The text of `parts`, the ones that are there, one after the other.
fn cat(parts: &[Option<&str>]) -> String {
    parts.iter().flatten().copied().collect()
}

fn opt(s: &Option<String>) -> Option<&str> {
    s.as_deref()
}

/// `s` if `cond`.
fn when(cond: bool, s: &str) -> Option<&str> {
    cond.then_some(s)
}

/// How deep types and symbols may nest in one another. The parser
/// recurses into each, and a name the program made up, `PAPAPA...H` for
/// a pointer to a pointer and so on, would otherwise take the host's
/// stack: 1,000 levels overflow a megabyte. Visual C++ 6's undecorator
/// recurses on the program's own stack; no name a compiler makes nests
/// anywhere near this deep, and one that does is given back as it is.
const MAX_DEPTH: u32 = 64;

struct Parser<'a> {
    flags: u32,
    text: &'a [u8],
    at: usize,
    /// The types and nested symbols being read, one in another.
    depth: u32,
    result: Option<String>,
    names: Strings,
    args: Strings,
    stack: Strings,
}

impl<'a> Parser<'a> {
    fn cur(&self) -> u8 {
        self.peek(0)
    }

    fn peek(&self, n: usize) -> u8 {
        self.text.get(self.at + n).copied().unwrap_or(0)
    }

    /// The character at the cursor, which moves past it.
    fn next(&mut self) -> u8 {
        let c = self.cur();
        self.at += 1;
        c
    }

    fn number(&mut self) -> Option<String> {
        let negative = self.cur() == b'?';
        if negative {
            self.at += 1;
        }
        let sign = if negative { "-" } else { "" };
        let c = self.cur();
        let text = match c {
            b'0'..=b'8' => {
                self.at += 1;
                format!("{}{}", sign, (c + 1) as char)
            }
            b'9' => {
                self.at += 1;
                format!("{}10", sign)
            }
            b'A'..=b'P' => {
                let mut value: u32 = 0;
                while (b'A'..=b'P').contains(&self.cur()) {
                    value = value
                        .wrapping_mul(16)
                        .wrapping_add((self.next() - b'A') as u32);
                }
                if self.cur() != b'@' {
                    return None;
                }
                self.at += 1;
                format!("{}{}", sign, value)
            }
            _ => return None,
        };
        Some(text)
    }

    /// The number of an array's dimensions. A compiler writes a byte or
    /// more for each, so a count past the bytes left isn't one it wrote,
    /// and the name isn't read: Wine's would add "[]" for every dimension
    /// it can't read, two billion of them for a count of 7FFFFFFFh.
    fn dimensions(&mut self) -> Option<usize> {
        let n = self.number()?;
        let count = n.parse::<i32>().unwrap_or(0).max(0) as usize;
        (count <= self.text.len().saturating_sub(self.at)).then_some(count)
    }

    /// A list of arguments, of a function (`z_term`, ended by Z) or a
    /// template (ended by @), in `open` and `close`.
    fn arguments(&mut self, z_term: bool, open: char, close: char) -> Option<String> {
        if z_term && self.cur() == b'X' && self.peek(1) == b'Z' {
            self.at += 2;
            return Some(format!("{}void{}", open, close));
        }
        let mut collected: Vec<String> = Vec::new();
        while self.cur() != 0 {
            let from = self.at;
            if self.cur() == b'@' {
                self.at += 1;
                break;
            }
            if z_term && self.cur() == b'Z' {
                self.at += 1;
                collected.push("...".to_string());
                break;
            }
            // An empty list of a variadic template.
            if !z_term && self.cur() == b'$' && self.peek(1) == b'$' && self.peek(2) == b'V' {
                self.at += 3;
                continue;
            }
            let ct = self.datatype(IN_ARGS)?;
            collected.push(cat(&[opt(&ct.left), opt(&ct.right)]));
            if z_term && self.at - from > 1 && self.args.num < 20 {
                self.args.push(ct.left.as_deref().unwrap_or(""));
                self.args.push(ct.right.as_deref().unwrap_or(""));
            }
        }
        // A function's list ends with Z: anything else means a type was
        // taken for something it isn't.
        if z_term && self.next() != b'Z' {
            return None;
        }
        if collected.is_empty() {
            return Some(format!("{}{}", open, close));
        }
        let mut rest: Option<String> = None;
        for arg in &collected[1..] {
            rest = Some(format!("{},{}", rest.as_deref().unwrap_or(""), arg));
        }
        let last = rest.as_deref().unwrap_or(&collected[0]);
        let space = if close == '>' && last.ends_with('>') {
            " "
        } else {
            ""
        };
        Some(format!(
            "{}{}{}{}{}",
            open,
            collected[0],
            rest.as_deref().unwrap_or(""),
            space,
            close
        ))
    }

    fn append_extended(&self, to: &mut Option<String>, s: &str, ms_keyword: bool) {
        if ms_keyword && self.flags & NO_MS_KEYWORDS != 0 {
            return;
        }
        let s = if ms_keyword && self.flags & NO_LEADING_UNDERSCORES != 0 {
            &s[2..]
        } else {
            s
        };
        let (before, after) = if ms_keyword { (" ", "") } else { ("", " ") };
        *to = Some(match to.take() {
            Some(t) => format!("{}{}{}{}", t, before, s, after),
            None => format!("{}{}", s, after),
        });
    }

    /// The modifiers before a qualifier: `__ptr64`, `__unaligned` and
    /// `__restrict`.
    fn extended_qualifier(&mut self) -> DataType {
        let mut xdt = DataType::default();
        let mut found = 0;
        loop {
            match self.cur() {
                b'E' => {
                    self.append_extended(&mut xdt.right, "__ptr64", true);
                    found |= 2;
                }
                b'F' => {
                    self.append_extended(&mut xdt.left, "__unaligned", true);
                    found |= 2;
                }
                b'I' => {
                    self.append_extended(&mut xdt.right, "__restrict", true);
                    found |= 2;
                }
                _ => {
                    if found == 1 || (found == 3 && self.flags & NO_MS_KEYWORDS != 0) {
                        xdt.flags = DT_NO_LRSEP_WS;
                    }
                    return xdt;
                }
            }
            self.at += 1;
        }
    }

    /// A const or volatile qualifier, of a member's class for Q to T,
    /// which goes into `class` (and can't go nowhere).
    fn qualifier(&mut self, xdt: &mut DataType, class: Option<&mut Option<String>>) -> bool {
        *xdt = self.extended_qualifier();
        let c = self.next();
        let qualifier = match c {
            b'A' | b'Q' => None,
            b'B' | b'R' => Some("const"),
            b'C' | b'S' => Some("volatile"),
            b'D' | b'T' => Some("const volatile"),
            _ => return false,
        };
        if let Some(q) = qualifier {
            xdt.flags &= !DT_NO_LRSEP_WS;
            xdt.left = Some(match xdt.left.take() {
                Some(left) => format!("{} {}", q, left),
                None => q.to_string(),
            });
        }
        if (b'Q'..=b'T').contains(&c) {
            let Some(name) = self.class_name() else {
                return false;
            };
            match class {
                Some(class) => *class = Some(name),
                None => return false,
            }
        } else if let Some(class) = class {
            *class = None;
        }
        true
    }

    fn function_qualifier(&mut self) -> Option<Option<String>> {
        let mut xdt = DataType::default();
        if !self.qualifier(&mut xdt, None) {
            return None;
        }
        if xdt.left.is_none() && xdt.right.is_none() {
            return Some(None);
        }
        let sep = if xdt.flags & DT_NO_LRSEP_WS != 0 {
            ""
        } else {
            " "
        };
        Some(Some(cat(&[opt(&xdt.left), Some(sep), opt(&xdt.right)])))
    }

    /// A pointer, a reference or a qualified type (`qualifier` says
    /// which) and what it refers to.
    fn qualified_type(&mut self, ct: &mut DataType, qualifier: u8, flags: u32) -> bool {
        let xdt1 = self.extended_qualifier();
        let (reference, str_qualifier) = match qualifier {
            b'A' => (Some(" &"), None),
            b'B' => (Some(" &"), Some(" volatile")),
            b'P' => (Some(" *"), None),
            b'Q' => (Some(" *"), Some(" const")),
            b'R' => (Some(" *"), Some(" volatile")),
            b'S' => (Some(" *"), Some(" const volatile")),
            b'?' => (None, None),
            b'$' => (Some(" &&"), None),
            _ => return false,
        };
        ct.right = None;
        ct.flags = 0;
        // Wine's reads $A as a managed handle (% and ^), which came after
        // Visual C++ 6: Microsoft's of then gives up on it.
        if self.cur() == b'$' && self.peek(1) == b'A' {
            return false;
        }
        let x1l = opt(&xdt1.left);
        let x1r = opt(&xdt1.right);
        let any = reference.is_some() || str_qualifier.is_some() || x1l.is_some() || x1r.is_some();
        let mut xdt2 = DataType::default();
        let mut class = None;
        if self.qualifier(&mut xdt2, Some(&mut class)) {
            let mark = self.stack.num;
            ct.left = if !any {
                None
            } else if let Some(class) = &class {
                // Wine's format has a place fewer than its arguments: the
                // qualifier of the pointer itself isn't shown.
                Some(cat(&[
                    when(x1l.is_some(), " "),
                    x1l,
                    Some(" "),
                    Some(class),
                    Some("::"),
                    reference.map(|r| &r[1..]),
                    when(x1r.is_some(), " "),
                    x1r,
                ]))
            } else {
                Some(cat(&[
                    when(x1l.is_some(), " "),
                    x1l,
                    reference,
                    when(x1r.is_some(), " "),
                    x1r,
                    str_qualifier,
                ]))
            };
            // A multidimensional array, which Microsoft's reads only
            // behind a pointer or a reference.
            if self.cur() == b'Y' && qualifier != b'?' {
                self.at += 1;
                let Some(count) = self.dimensions() else {
                    return false;
                };
                let inner = match (&ct.left, &xdt2.left) {
                    (Some(left), None) => left.get(1..).unwrap_or("").to_string(),
                    (left, _) => left.clone().unwrap_or_default(),
                };
                ct.left = Some(format!(" ({}{}", xdt2.left.as_deref().unwrap_or(""), inner));
                let mut right = ")".to_string();
                xdt2.left = None;
                for _ in 0..count {
                    let n = self.number();
                    right = format!("{}[{}]", right, n.as_deref().unwrap_or(""));
                }
                ct.right = Some(right);
            }
            let Some(sub) = self.datatype(0) else {
                return false;
            };
            if sub.flags & DT_NO_LEADING_WS != 0
                && let Some(left) = &mut ct.left
                && !left.is_empty()
            {
                left.remove(0);
            }
            let x2l = opt(&xdt2.left);
            ct.left = Some(cat(&[
                opt(&sub.left),
                when(x2l.is_some(), " "),
                x2l,
                opt(&ct.left),
                when(
                    (x2l.is_some() || str_qualifier.is_some()) && flags & WS_AFTER_QUAL_IF != 0,
                    " ",
                ),
            ]));
            if let Some(sub_right) = &sub.right {
                ct.right = Some(cat(&[opt(&ct.right), Some(sub_right)]));
            }
            self.stack.num = mark;
        } else if any {
            ct.left = Some(cat(&[
                when(x1l.is_some(), " "),
                x1l,
                reference,
                when(x1r.is_some(), " "),
                x1r,
                str_qualifier,
            ]));
        } else {
            ct.left = None;
        }
        true
    }

    /// A name up to its @, which goes into `names`.
    fn literal(&mut self) -> Option<String> {
        let from = self.at;
        loop {
            let c = self.cur();
            if !(c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$' | b'<' | b'>')) {
                return None;
            }
            self.at += 1;
            if self.cur() == b'@' {
                break;
            }
        }
        self.at += 1;
        let name = String::from_utf8_lossy(&self.text[from..self.at - 1]).into_owned();
        self.names.push(&name);
        self.names.get(self.names.num - self.names.start - 1)
    }

    /// A name with its template arguments. Back references in the
    /// arguments count from the template's own name, and what the
    /// arguments put in the tables goes again at their end.
    fn template_name(&mut self) -> Option<String> {
        let num_mark = self.names.num;
        let start_mark = self.names.start;
        let stack_mark = self.stack.num;
        let args_mark = self.args.num;
        self.names.start = self.names.num;
        let Some(mut name) = self.literal() else {
            self.names.start = start_mark;
            return None;
        };
        // Microsoft's gives up on arguments it can't read, where Wine's
        // leaves them out.
        let args = self.arguments(false, '<', '>');
        self.names.num = num_mark;
        self.names.start = start_mark;
        self.stack.num = stack_mark;
        self.args.num = args_mark;
        name.push_str(&args?);
        Some(name)
    }

    /// The parts of a qualified name, innermost first, up to its @, onto
    /// `stack`.
    fn class(&mut self) -> bool {
        // As Wine's, a nested name that fails keeps the part before it.
        let mut name: Option<String> = None;
        while self.cur() != b'@' {
            match self.cur() {
                0 => return false,
                c @ b'0'..=b'9' => {
                    self.at += 1;
                    name = self.names.get((c - b'0') as usize);
                }
                b'?' => {
                    self.at += 1;
                    match self.cur() {
                        b'$' => {
                            self.at += 1;
                            name = self.template_name();
                            if let Some(n) = &name {
                                self.names.push(n);
                            }
                        }
                        // Wine's reads ?A0x...@ as `anonymous namespace',
                        // which Microsoft's of Visual C++ 6 doesn't know.
                        b'?' => {
                            if let Some(symbol) = self.nested_symbol() {
                                name = Some(format!("`{}'", symbol));
                            }
                        }
                        _ => {
                            let Some(n) = self.number() else {
                                return false;
                            };
                            name = Some(format!("`{}'", n));
                        }
                    }
                }
                _ => name = self.literal(),
            }
            match &name {
                Some(n) => {
                    let n = n.clone();
                    self.stack.push(&n);
                }
                None => return false,
            }
        }
        self.at += 1;
        true
    }

    /// A whole decorated name inside this one: the function a local class
    /// is in, or a template's argument that is the address of a symbol.
    /// Its own arguments show, even in a type alone.
    fn nested_symbol(&mut self) -> Option<String> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let symbol = self.nested_symbol_here();
        self.depth -= 1;
        symbol
    }

    fn nested_symbol_here(&mut self) -> Option<String> {
        let stack = std::mem::take(&mut self.stack);
        let (start, num) = (self.names.start, self.names.num);
        let (flags, result) = (self.flags, self.result.take());
        self.flags &= !NO_ARGUMENTS;
        let symbol = if self.demangle() {
            self.result.take()
        } else {
            None
        };
        self.names.start = start;
        self.names.num = num;
        self.stack = stack;
        self.flags = flags;
        self.result = result;
        symbol
    }

    /// The parts on `stack` from `start`, outermost first, joined by ::.
    fn class_string(&self, start: usize) -> String {
        let parts: Vec<&str> = self.stack.elts[start..self.stack.num]
            .iter()
            .rev()
            .map(String::as_str)
            .collect();
        parts.join("::")
    }

    fn class_name(&mut self) -> Option<String> {
        let mark = self.stack.num;
        let name = self.class().then(|| self.class_string(mark));
        self.stack.num = mark;
        name
    }

    /// The calling convention `c` names, and whether it is exported.
    /// None for an unknown one.
    fn calling_convention(
        c: u8,
        flags: u32,
    ) -> Option<(Option<&'static str>, Option<&'static str>)> {
        if flags & (NO_MS_KEYWORDS | NO_ALLOCATION_LANGUAGE) != 0 {
            return Some((None, None));
        }
        let short = flags & NO_LEADING_UNDERSCORES != 0;
        let exported = (c.wrapping_sub(b'A') % 2 == 1).then_some(if short {
            "dll_export "
        } else {
            "__dll_export "
        });
        let convention = match c {
            b'A' | b'B' => Some("__cdecl"),
            b'C' | b'D' => Some("__pascal"),
            b'E' | b'F' => Some("__thiscall"),
            b'G' | b'H' => Some("__stdcall"),
            b'I' | b'J' => Some("__fastcall"),
            b'K' | b'L' => None,
            b'M' => Some("__clrcall"),
            _ => return None,
        };
        Some((
            convention.map(|c| if short { &c[2..] } else { c }),
            exported,
        ))
    }

    fn simple_type(c: u8) -> Option<&'static str> {
        Some(match c {
            b'C' => "signed char",
            b'D' => "char",
            b'E' => "unsigned char",
            b'F' => "short",
            b'G' => "unsigned short",
            b'H' => "int",
            b'I' => "unsigned int",
            b'J' => "long",
            b'K' => "unsigned long",
            b'M' => "float",
            b'N' => "double",
            b'O' => "long double",
            b'X' => "void",
            _ => return None,
        })
    }

    fn extended_type(c: u8) -> Option<&'static str> {
        Some(match c {
            b'D' => "__int8",
            b'E' => "unsigned __int8",
            b'F' => "__int16",
            b'G' => "unsigned __int16",
            b'H' => "__int32",
            b'I' => "unsigned __int32",
            b'J' => "__int64",
            b'K' => "unsigned __int64",
            b'L' => "__int128",
            b'M' => "unsigned __int128",
            b'N' => "bool",
            b'Q' => "char8_t",
            b'S' => "char16_t",
            b'U' => "char32_t",
            b'W' => "wchar_t",
            _ => return None,
        })
    }

    /// A function type's calling convention, return type and arguments.
    fn function_signature(&mut self) -> Option<Signature> {
        let mark = self.stack.num;
        let c = self.next();
        let (call_conv, _) = Self::calling_convention(c, self.flags & !NO_ALLOCATION_LANGUAGE)?;
        let return_ct = self.datatype(0)?;
        let arguments = self.arguments(true, '(', ')')?;
        self.stack.num = mark;
        Some(Signature {
            call_conv,
            return_ct,
            arguments,
        })
    }

    /// A type; None if it isn't one, or if it is nested deeper than
    /// `MAX_DEPTH`.
    fn datatype(&mut self, flags: u32) -> Option<DataType> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let ct = self.datatype_here(flags);
        self.depth -= 1;
        ct
    }

    fn datatype_here(&mut self, flags: u32) -> Option<DataType> {
        let mut ct = DataType::default();
        let dt = self.next();
        match dt {
            b'_' => {
                let c = self.next();
                ct.left = Self::extended_type(c).map(str::to_string);
            }
            b'C' | b'D' | b'E' | b'F' | b'G' | b'H' | b'I' | b'J' | b'K' | b'M' | b'N' | b'O'
            | b'X' | b'Z' => {
                ct.left = Self::simple_type(dt).map(str::to_string);
            }
            // Wine's takes Y for a cointerface too, which Microsoft's of
            // Visual C++ 6 doesn't know.
            b'T' | b'U' | b'V' => {
                let name = self.class_name()?;
                let kind = if self.flags & NO_COMPLEX_TYPE != 0 {
                    ""
                } else {
                    match dt {
                        b'T' => "union ",
                        b'U' => "struct ",
                        _ => "class ",
                    }
                };
                ct.left = Some(format!("{}{}", kind, name));
            }
            b'?' => {
                if flags & IN_ARGS != 0 {
                    let n = self.number()?;
                    ct.left = Some(format!("`template-parameter-{}'", n));
                } else if !self.qualified_type(&mut ct, b'?', flags) {
                    return None;
                } else {
                    ct.flags |= DT_BY_VALUE;
                }
            }
            b'A' | b'B' => {
                if !self.qualified_type(&mut ct, dt, flags) {
                    return None;
                }
            }
            b'P' | b'Q' | b'R' | b'S' => {
                let dt = if flags & IN_ARGS == 0 { b'P' } else { dt };
                if self.cur().is_ascii_digit() {
                    let pointer_qualifier = match dt {
                        b'Q' => Some("const"),
                        b'R' => Some("volatile"),
                        b'S' => Some("const volatile"),
                        _ => None,
                    };
                    match self.cur() {
                        b'8' => {
                            // A pointer to a member function.
                            self.at += 1;
                            let class = self.class_name()?;
                            let qualifier = self.function_qualifier()?;
                            let fs = self.function_signature()?;
                            ct.left = Some(cat(&[
                                opt(&fs.return_ct.left),
                                opt(&fs.return_ct.right),
                                Some(" ("),
                                fs.call_conv,
                                Some(" "),
                                Some(&class),
                                Some("::*"),
                                pointer_qualifier,
                            ]));
                            ct.right =
                                Some(cat(&[Some(")"), Some(&fs.arguments), opt(&qualifier)]));
                            ct.flags = DT_FUNCTION;
                        }
                        b'6' => {
                            // A pointer to a function.
                            self.at += 1;
                            let fs = self.function_signature()?;
                            ct.left = Some(cat(&[
                                opt(&fs.return_ct.left),
                                opt(&fs.return_ct.right),
                                Some(" ("),
                                fs.call_conv,
                                Some("*"),
                                pointer_qualifier,
                            ]));
                            ct.flags = DT_NO_LEADING_WS | DT_FUNCTION;
                            ct.right = Some(format!("){}", fs.arguments));
                        }
                        _ => return None,
                    }
                } else if !self.qualified_type(&mut ct, dt, flags) {
                    return None;
                }
            }
            b'W' => {
                if self.cur() != b'4' {
                    return None;
                }
                self.at += 1;
                let name = self.class_name()?;
                ct.left = Some(if self.flags & NO_COMPLEX_TYPE != 0 {
                    name
                } else {
                    format!("enum {}", name)
                });
            }
            b'0'..=b'9' => {
                // A type seen before, its left and right parts.
                let i = (dt - b'0') as usize * 2;
                ct.left = self.args.get(i);
                ct.right = self.args.get(i + 1);
                ct.left.as_ref()?;
            }
            b'$' => match self.next() {
                b'0' => ct.left = Some(self.number()?),
                b'1' => {
                    // The address of a function or a variable.
                    if self.cur() != b'?' {
                        return None;
                    }
                    ct.left = Some(format!("&{}", self.nested_symbol()?));
                }
                b'D' => {
                    let n = self.number()?;
                    ct.left = Some(format!("`template-parameter{}'", n));
                }
                b'F' => {
                    let p1 = self.number()?;
                    let p2 = self.number()?;
                    ct.left = Some(format!("{{{},{}}}", p1, p2));
                }
                b'G' => {
                    let p1 = self.number()?;
                    let p2 = self.number()?;
                    let p3 = self.number()?;
                    ct.left = Some(format!("{{{},{},{}}}", p1, p2, p3));
                }
                b'Q' => {
                    let n = self.number()?;
                    ct.left = Some(format!("`non-type-template-parameter{}'", n));
                }
                b'$' => match self.cur() {
                    b'A' => {
                        self.at += 1;
                        if self.cur() == b'6' {
                            self.at += 1;
                            let fs = self.function_signature()?;
                            ct.left = Some(cat(&[
                                opt(&fs.return_ct.left),
                                opt(&fs.return_ct.right),
                                Some(" "),
                                fs.call_conv,
                                Some(&fs.arguments),
                            ]));
                        }
                    }
                    b'B' => {
                        let mark = self.stack.num;
                        self.at += 1;
                        let mut array: Option<String> = None;
                        if self.cur() == b'Y' {
                            self.at += 1;
                            let count = self.dimensions()?;
                            for _ in 0..count {
                                let n = self.number();
                                array = Some(format!(
                                    "{}[{}]",
                                    array.as_deref().unwrap_or(""),
                                    n.as_deref().unwrap_or("")
                                ));
                            }
                        }
                        let sub = self.datatype(0)?;
                        ct.left = match &array {
                            Some(array) => {
                                Some(format!("{} {}", sub.left.as_deref().unwrap_or(""), array))
                            }
                            None => sub.left,
                        };
                        ct.right = sub.right;
                        self.stack.num = mark;
                    }
                    b'C' => {
                        self.at += 1;
                        let mut xdt = DataType::default();
                        if !self.qualifier(&mut xdt, None) {
                            return None;
                        }
                        ct = self.datatype(flags)?;
                        ct.left = Some(format!(
                            "{} {}",
                            ct.left.as_deref().unwrap_or(""),
                            xdt.left.as_deref().unwrap_or("")
                        ));
                    }
                    b'Q' => {
                        self.at += 1;
                        if !self.qualified_type(&mut ct, b'$', flags) {
                            return None;
                        }
                    }
                    b'T' => {
                        self.at += 1;
                        ct.left = Some("std::nullptr_t".to_string());
                    }
                    _ => {}
                },
                _ => {}
            },
            _ => {}
        }
        ct.left.is_some().then_some(ct)
    }

    /// A variable or a static member.
    fn data(&mut self) -> bool {
        let c = self.cur();
        let access = if self.flags & NO_ACCESS_SPECIFIERS == 0 {
            match c {
                b'0' => Some("private: "),
                b'1' => Some("protected: "),
                b'2' => Some("public: "),
                _ => None,
            }
        } else {
            None
        };
        let member_type =
            (self.flags & NO_MEMBER_TYPE == 0 && (b'0'..=b'2').contains(&c)).then_some("static ");
        let name = self.class_string(0);
        let mut ct = DataType::default();
        let mut xdt = DataType::default();
        match self.next() {
            b'0'..=b'5' => {
                let mark = self.stack.num;
                let Some(t) = self.datatype(0) else {
                    return false;
                };
                ct = t;
                let mut class = None;
                if !self.qualifier(&mut xdt, Some(&mut class)) {
                    return false;
                }
                self.stack.num = mark;
            }
            b'6' | b'7' => {
                // A static the compiler made: a vftable and the like.
                if !self.qualifier(&mut xdt, None) {
                    return false;
                }
                if self.cur() != b'@' {
                    let Some(class) = self.class_name() else {
                        return false;
                    };
                    ct.right = Some(format!("{{for `{}'}}", class));
                }
                if self.cur() == b'@' {
                    self.at += 1;
                }
            }
            b'8' | b'9' => {
                xdt.left = None;
            }
            _ => return false,
        }
        if self.flags & NAME_ONLY != 0 {
            ct.left = None;
            ct.right = None;
            xdt.left = None;
            xdt.right = None;
        }
        let (cl, xl, xr) = (opt(&ct.left), opt(&xdt.left), opt(&xdt.right));
        // Microsoft's puts two spaces before the name, "int  a", or one
        // after a pointer to a function, "int (__cdecl* a)(int)", and a
        // __ptr64 of the variable itself after them.
        let gap = if ct.flags & DT_FUNCTION != 0 {
            " "
        } else {
            "  "
        };
        self.result = Some(cat(&[
            access,
            member_type,
            cl,
            when(xl.is_some() && cl.is_some(), " "),
            xl,
            when(xl.is_some() || cl.is_some(), gap),
            xr,
            when(xr.is_some(), " "),
            Some(&name),
            opt(&ct.right),
        ]));
        true
    }

    /// A function or a method.
    fn method(&mut self, cast_operator: bool) -> bool {
        let accmem = self.next();
        let mut access_id: i32 = -1;
        if accmem == b'$' {
            let c = self.cur();
            if (b'0'..=b'5').contains(&c) {
                access_id = ((c - b'0') / 2) as i32;
            } else if c != b'B' {
                // Wine's reads $R as a vtordispex thunk, which came after
                // Visual C++ 6.
                return false;
            }
        } else if accmem.is_ascii_uppercase() {
            access_id = ((accmem - b'A') / 8) as i32;
        } else {
            return false;
        }
        // As C computes it: negative for '$'.
        let kind = (accmem as i32 - b'A' as i32) % 8;
        let thunk = accmem == b'$' || kind == 6 || kind == 7;
        let mut access: Option<String> = match access_id {
            0 => Some("private: ".into()),
            1 => Some("protected: ".into()),
            2 => Some("public: ".into()),
            _ => None,
        };
        if thunk {
            access = Some(format!("[thunk]:{}", access.as_deref().unwrap_or(" ")));
        }
        let mut member_type = None;
        if accmem == b'$' && self.cur() != b'B' {
            member_type = Some("virtual ");
        } else if accmem <= b'X' {
            match kind {
                2 | 3 => member_type = Some("static "),
                4..=7 => member_type = Some("virtual "),
                _ => {}
            }
        }
        if self.flags & NO_ACCESS_SPECIFIERS != 0 {
            access = None;
        }
        if self.flags & NO_MEMBER_TYPE != 0 {
            member_type = None;
        }
        let mut name = self.class_string(0);
        let mut has_args = true;
        let mut has_ret = true;
        if accmem == b'$' && self.cur() == b'B' {
            // A vcall thunk.
            self.at += 1;
            let n = self.number();
            let Some(n) = n else {
                return false;
            };
            if self.next() != b'A' {
                return false;
            }
            name = format!("{}{{{},{{flat}}}}' ", name, n);
            has_args = false;
            has_ret = false;
        } else if accmem == b'$' {
            // A vtordisp thunk.
            self.at += 1;
            let n1 = self.number();
            let n2 = self.number();
            let (Some(n1), Some(n2)) = (n1, n2) else {
                return false;
            };
            name = format!("{}`vtordisp{{{},{}}}' ", name, n1, n2);
        } else if kind == 6 || kind == 7 {
            let n = self.number();
            name = format!("{}`adjustor{{{}}}' ", name, n.as_deref().unwrap_or(""));
        }
        let mut function_qualifier = None;
        if has_args && (accmem == b'$' || (accmem <= b'X' && kind != 2 && kind != 3)) {
            // The implicit `this`.
            match self.function_qualifier() {
                Some(q) => function_qualifier = q,
                None => return false,
            }
        }
        let c = self.next();
        let Some((call_conv, exported)) = Self::calling_convention(c, self.flags) else {
            return false;
        };
        let mut ret = DataType::default();
        if has_ret && self.cur() == b'@' {
            ret.left = Some("void".into());
            self.at += 1;
        } else if has_ret {
            match self.datatype(if cast_operator { WS_AFTER_QUAL_IF } else { 0 }) {
                Some(t) => ret = t,
                None => return false,
            }
        }
        if !has_ret || self.flags & NO_FUNCTION_RETURNS != 0 {
            ret = DataType::default();
        }
        if cast_operator {
            name = format!(
                "{} {}{}",
                name,
                ret.left.as_deref().unwrap_or(""),
                ret.right.as_deref().unwrap_or("")
            );
            ret = DataType::default();
        }
        let mark = self.stack.num;
        let mut args = None;
        if has_args {
            match self.arguments(true, '(', ')') {
                Some(a) => args = Some(a),
                None => return false,
            }
        }
        if self.flags & NAME_ONLY != 0 {
            args = None;
            function_qualifier = None;
        }
        if self.flags & NO_THISTYPE != 0 {
            function_qualifier = None;
        }
        self.stack.num = mark;
        let (rl, rr) = (opt(&ret.left), opt(&ret.right));
        self.result = Some(cat(&[
            access.as_deref(),
            member_type,
            rl,
            when(rl.is_some() && rr.is_none(), " "),
            when(
                rl.is_some() && rr.is_none() && ret.flags & DT_BY_VALUE != 0,
                " ",
            ),
            call_conv,
            when(call_conv.is_some(), " "),
            exported,
            Some(&name),
            opt(&args),
            opt(&function_qualifier),
            rr,
        ]));
        true
    }

    /// A whole decorated name, into `result`.
    fn demangle(&mut self) -> bool {
        if self.flags & NO_ARGUMENTS != 0 {
            // A type alone, as type_info::name asks for.
            if let Some(ct) = self.datatype(0) {
                self.result = Some(cat(&[opt(&ct.left), opt(&ct.right)]));
                return true;
            }
            return false;
        }
        if self.cur() != b'?' {
            return false;
        }
        self.at += 1;
        #[derive(PartialEq)]
        enum Post {
            None,
            Constructor,
            Destructor,
            Cast,
        }
        let mut post = Post::None;
        if self.cur() == b'?' {
            self.at += 1;
            let function_name: String = match self.cur() {
                b'0' => {
                    post = Post::Constructor;
                    String::new()
                }
                b'1' => {
                    post = Post::Destructor;
                    String::new()
                }
                b'B' => {
                    post = Post::Cast;
                    "operator".into()
                }
                b'_' => {
                    self.at += 1;
                    match self.cur() {
                        b'C' => {
                            // A string literal, which Microsoft's never
                            // undecorates.
                            self.result = Some("`string'".into());
                            let ok = self.text.len() > self.at && self.text.last() == Some(&b'@');
                            self.at = self.text.len();
                            return ok;
                        }
                        b'R' => {
                            self.flags |= NO_FUNCTION_RETURNS;
                            self.at += 1;
                            match self.cur() {
                                b'0' => {
                                    self.at += 1;
                                    let Some(ct) = self.datatype(0) else {
                                        return false;
                                    };
                                    self.at -= 1;
                                    format!(
                                        "{}{} `RTTI Type Descriptor'",
                                        ct.left.as_deref().unwrap_or(""),
                                        ct.right.as_deref().unwrap_or("")
                                    )
                                }
                                b'1' => {
                                    self.at += 1;
                                    let n: Vec<String> =
                                        (0..4).map(|_| self.number().unwrap_or_default()).collect();
                                    self.at -= 1;
                                    format!(
                                        "`RTTI Base Class Descriptor at ({},{},{},{})'",
                                        n[0], n[1], n[2], n[3]
                                    )
                                }
                                b'2' => "`RTTI Base Class Array'".into(),
                                b'3' => "`RTTI Class Hierarchy Descriptor'".into(),
                                b'4' => "`RTTI Complete Object Locator'".into(),
                                // Wine goes on with no name.
                                _ => String::new(),
                            }
                        }
                        b'_' => {
                            self.at += 1;
                            if self.cur() != b'K' {
                                return false;
                            }
                            self.at += 1;
                            let literal = self.literal();
                            self.at -= 1;
                            format!("operator \"\" {}", literal.as_deref().unwrap_or(""))
                        }
                        c => match underscore_operator(c) {
                            Some(name) => name.into(),
                            None => return false,
                        },
                    }
                }
                // Microsoft's of Visual C++ 6 doesn't know the names of
                // function templates (??$), which came later.
                c => match operator(c) {
                    Some(name) => name.into(),
                    None => return false,
                },
            };
            self.at += 1;
            self.stack.push(&function_name);
        } else if self.cur() == b'$' {
            // A name with template arguments and nothing more.
            self.at += 1;
            self.result = self.template_name();
            return self.result.is_some();
        }
        // The class the function is a member of, up to its @@, or @ for
        // none.
        match self.cur() {
            b'@' => self.at += 1,
            b'$' => {}
            _ => {
                if !self.class() {
                    return false;
                }
            }
        }
        match post {
            Post::Constructor | Post::Destructor => {
                if self.stack.num <= 1 {
                    return false;
                }
                // In Microsoft's, the function's own name takes the first
                // back reference, before the class's: ??0C@N@@QAE@ABV01@@Z
                // is N::C::C(class C::C const &).
                let own = self.stack.elts[1].clone();
                let at = self.names.start;
                self.names.elts.insert(at, own);
                self.names.num += 1;
                self.names.max += 1;
                let tilde = if post == Post::Destructor { "~" } else { "" };
                self.stack.elts[0] =
                    format!("{}{}{}", tilde, self.stack.elts[1], self.stack.elts[0]);
                self.flags |= NO_FUNCTION_RETURNS;
            }
            Post::Cast => self.flags &= !NO_FUNCTION_RETURNS,
            Post::None => {}
        }
        match self.cur() {
            b'0'..=b'9' => self.data(),
            b'A'..=b'Z' | b'$' => self.method(post == Post::Cast),
            _ => false,
        }
    }
}

struct Signature {
    call_conv: Option<&'static str>,
    return_ct: DataType,
    arguments: String,
}

/// The operator ?c names.
fn operator(c: u8) -> Option<&'static str> {
    Some(match c {
        b'2' => "operator new",
        b'3' => "operator delete",
        b'4' => "operator=",
        b'5' => "operator>>",
        b'6' => "operator<<",
        b'7' => "operator!",
        b'8' => "operator==",
        b'9' => "operator!=",
        b'A' => "operator[]",
        b'C' => "operator->",
        b'D' => "operator*",
        b'E' => "operator++",
        b'F' => "operator--",
        b'G' => "operator-",
        b'H' => "operator+",
        b'I' => "operator&",
        b'J' => "operator->*",
        b'K' => "operator/",
        b'L' => "operator%",
        b'M' => "operator<",
        b'N' => "operator<=",
        b'O' => "operator>",
        b'P' => "operator>=",
        b'Q' => "operator,",
        b'R' => "operator()",
        b'S' => "operator~",
        b'T' => "operator^",
        b'U' => "operator|",
        b'V' => "operator&&",
        b'W' => "operator||",
        b'X' => "operator*=",
        b'Y' => "operator+=",
        b'Z' => "operator-=",
        _ => return None,
    })
}

/// The operator or special name ?_c names.
fn underscore_operator(c: u8) -> Option<&'static str> {
    Some(match c {
        b'0' => "operator/=",
        b'1' => "operator%=",
        b'2' => "operator>>=",
        b'3' => "operator<<=",
        b'4' => "operator&=",
        b'5' => "operator|=",
        b'6' => "operator^=",
        b'7' => "`vftable'",
        b'8' => "`vbtable'",
        b'9' => "`vcall'",
        b'A' => "`typeof'",
        b'B' => "`local static guard'",
        b'D' => "`vbase destructor'",
        b'E' => "`vector deleting destructor'",
        b'F' => "`default constructor closure'",
        b'G' => "`scalar deleting destructor'",
        b'H' => "`vector constructor iterator'",
        b'I' => "`vector destructor iterator'",
        b'J' => "`vector vbase constructor iterator'",
        b'K' => "`virtual displacement map'",
        b'L' => "`eh vector constructor iterator'",
        b'M' => "`eh vector destructor iterator'",
        b'N' => "`eh vector vbase constructor iterator'",
        b'O' => "`copy constructor closure'",
        b'S' => "`local vftable'",
        b'T' => "`local vftable constructor closure'",
        b'U' => "operator new[]",
        b'V' => "operator delete[]",
        b'X' => "`placement delete closure'",
        b'Y' => "`placement delete[] closure'",
        _ => return None,
    })
}

/// `mangled` undecorated as `__unDNameEx` does with `flags`, or None
/// where it gives back the name as it was.
pub fn undecorate(mangled: &[u8], flags: u32) -> Option<String> {
    let mut flags = flags & 0xFFFF;
    if flags & NAME_ONLY != 0 {
        flags |= NO_FUNCTION_RETURNS
            | NO_ACCESS_SPECIFIERS
            | NO_MEMBER_TYPE
            | NO_ALLOCATION_LANGUAGE
            | NO_COMPLEX_TYPE;
    }
    let mut parser = Parser {
        flags,
        text: mangled,
        at: 0,
        depth: 0,
        result: None,
        names: Strings::default(),
        args: Strings::default(),
        stack: Strings::default(),
    };
    // Microsoft's gives up on a name with text past its end, which Wine's
    // ignores.
    if parser.demangle() && parser.at >= mangled.len() {
        parser.result
    } else {
        None
    }
}

/// What `type_info::name` gives for the type whose decorated name, past
/// its first character, is `mangled`: the undecorated type, with the
/// spaces at its end taken off.
pub fn type_name(mangled: &[u8]) -> Vec<u8> {
    let mut text = match undecorate(mangled, TYPE_NAME) {
        Some(name) => name.into_bytes(),
        None => mangled.to_vec(),
    };
    while text.last() == Some(&b' ') {
        text.pop();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every name of undname_vc6.txt undecorates as Microsoft's
    /// undecorator of Visual C++ 6 undecorates it.
    #[test]
    fn names_undecorate_as_visual_cpp_6s() {
        let table = include_str!("undname_vc6.txt");
        let mut wrong = Vec::new();
        let mut count = 0;
        for line in table.lines().filter(|l| !l.starts_with('#')) {
            let [flags, mangled, want] = line.split('\t').collect::<Vec<_>>()[..] else {
                panic!("not a line of the table: {:?}", line);
            };
            let flags = u32::from_str_radix(flags, 16).unwrap();
            let got = undecorate(mangled.as_bytes(), flags).unwrap_or_else(|| mangled.to_string());
            if got != want {
                let at = format!("{:X} {}", flags, mangled);
                wrong.push(format!("{at}\n  want {want:?}\n  got  {got:?}"));
            }
            count += 1;
        }
        assert!(count > 300);
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// A pointer to a pointer `n` times, and a template of a template `n`
    /// times.
    fn pointers(n: usize) -> Vec<u8> {
        let mut name = b"PA".repeat(n);
        name.push(b'H');
        name
    }

    fn templates(n: usize) -> Vec<u8> {
        let mut name = b"?AV".to_vec();
        name.extend(b"?$A@V".repeat(n));
        name.extend_from_slice(b"B@@");
        name.extend(b"@@".repeat(n));
        name
    }

    /// Names nested deeper than MAX_DEPTH are given back as they are, on
    /// a stack of a megabyte, the size of the host's main thread on
    /// Windows, which a thousand levels overflowed. Ones a compiler makes
    /// still undecorate.
    #[test]
    fn deep_names_leave_the_stack_alone() {
        let results = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(|| {
                [pointers(2000), templates(800), pointers(20), templates(20)]
                    .map(|name| undecorate(&name, TYPE_NAME))
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(results[0], None);
        assert_eq!(results[1], None);
        assert!(
            results[2]
                .as_deref()
                .is_some_and(|s| s.starts_with("int *"))
        );
        let template = format!("class {}B>{}", "A<class ".repeat(20), " >".repeat(19));
        assert_eq!(results[3].as_deref(), Some(template.as_str()));
    }

    /// An array whose count of dimensions is past the name's end isn't
    /// read, however large the count.
    #[test]
    fn a_dimension_count_past_the_end_is_no_array() {
        assert_eq!(undecorate(b"PAYHPPPPPPP@H", TYPE_NAME), None);
        assert_eq!(undecorate(b"$$BYHPPPPPPP@H", TYPE_NAME), None);
    }

    /// type_info::name's text: the type, with no spaces at its end, or
    /// the name as it is when it isn't one.
    #[test]
    fn type_names() {
        assert_eq!(
            type_name(b"?AV?$T@V?$U@H@@@@"),
            b"class T<class U<int> >".to_vec()
        );
        assert_eq!(type_name(b"?AUS@N@@"), b"struct N::S".to_vec());
        assert_eq!(type_name(b"PAH"), b"int *".to_vec());
        assert_eq!(type_name(b"?AVC@@X"), b"?AVC@@X".to_vec());
    }
}
