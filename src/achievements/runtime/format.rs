//! How values are shown: leaderboard scores and times, and the rich
//! presence's numbers.

use super::typed::{Kind, Typed};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Frames,
    Seconds,
    Centisecs,
    Score,
    Value,
    Minutes,
    SecondsAsMinutes,
    Float(u8),
    Fixed(u8),
    Tens,
    Hundreds,
    Thousands,
    UnsignedValue,
    Unformatted,
}

impl Format {
    /// A leaderboard's `Format` or a rich presence's `FormatType=`.
    pub fn parse(name: &str) -> Format {
        match name {
            "FRAMES" | "TIME" => Format::Frames,
            "SECS" | "TIMESECS" => Format::Seconds,
            "MILLISECS" => Format::Centisecs,
            "SCORE" | "POINTS" | "OTHER" => Format::Score,
            "MINUTES" => Format::Minutes,
            "SECS_AS_MINS" => Format::SecondsAsMinutes,
            "TENS" => Format::Tens,
            "HUNDREDS" => Format::Hundreds,
            "THOUSANDS" => Format::Thousands,
            "UNSIGNED" => Format::UnsignedValue,
            _ => {
                if let Some(n) = name
                    .strip_prefix("FLOAT")
                    .and_then(one_digit)
                    .filter(|n| (1..=6).contains(n))
                {
                    Format::Float(n)
                } else if let Some(n) = name
                    .strip_prefix("FIXED")
                    .and_then(one_digit)
                    .filter(|n| (1..=3).contains(n))
                {
                    Format::Fixed(n)
                } else {
                    Format::Value
                }
            }
        }
    }

    /// `value` shown in this format.
    pub fn show(self, value: Typed) -> String {
        let signed = || value.converted(Kind::Signed).i32();
        let unsigned = || value.converted(Kind::Unsigned).u32();
        let text = match self {
            Format::Value => signed().to_string(),
            // Sixty frames a second.
            Format::Frames => centiseconds(unsigned().wrapping_mul(10) / 6),
            Format::Centisecs => centiseconds(unsigned()),
            Format::Seconds => seconds(unsigned()),
            Format::SecondsAsMinutes => minutes(unsigned() / 60),
            Format::Minutes => minutes(unsigned()),
            Format::Score => return format!("{:06}", signed()),
            Format::Float(digits) => {
                let f = value.converted(Kind::Float).f32();
                let sign = if f.is_sign_negative() { "-" } else { "" };
                if f.is_nan() {
                    format!("{}nan", sign)
                } else if f.is_infinite() {
                    format!("{}inf", sign)
                } else {
                    format!("{:.*}", digits as usize, f)
                }
            }
            Format::Fixed(digits) => {
                let v = signed();
                let factor = 10i32.pow(digits as u32);
                // As C prints it, the smallest integer's too.
                let fraction = if v >= 0 {
                    v % factor
                } else {
                    v.wrapping_neg() % factor
                } as u32;
                format!("{}.{:0w$}", v / factor, fraction, w = digits as usize)
            }
            Format::Tens => padded(signed(), "0"),
            Format::Hundreds => padded(signed(), "00"),
            Format::Thousands => padded(signed(), "000"),
            Format::UnsignedValue => unsigned().to_string(),
            Format::Unformatted => return unsigned().to_string(),
        };
        insert_commas(text)
    }
}

fn one_digit(s: &str) -> Option<u8> {
    let mut chars = s.chars();
    let d = chars.next()?.to_digit(10)?;
    chars.next().is_none().then_some(d as u8)
}

fn minutes(minutes: u32) -> String {
    format!("{}h{:02}", minutes / 60, minutes % 60)
}

fn seconds(seconds: u32) -> String {
    let minutes = seconds / 60;
    if minutes < 60 {
        format!("{}:{:02}", minutes, seconds % 60)
    } else {
        format!("{}h{:02}:{:02}", minutes / 60, minutes % 60, seconds % 60)
    }
}

fn centiseconds(centiseconds: u32) -> String {
    format!("{}.{:02}", seconds(centiseconds / 100), centiseconds % 100)
}

fn padded(value: i32, zeros: &str) -> String {
    if value == 0 {
        "0".to_string()
    } else {
        format!("{}{}", value, zeros)
    }
}

/// Thousands separated by commas in the leading number.
fn insert_commas(text: String) -> String {
    let sign = if text.starts_with('-') { 1 } else { 0 };
    let digits = text[sign..].bytes().take_while(u8::is_ascii_digit).count();
    if digits <= 3 {
        return text;
    }
    let (number, rest) = text[sign..].split_at(digits);
    let mut out = text[..sign].to_string();
    for (i, c) in number.chars().enumerate() {
        if i > 0 && (digits - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_show_as_rcheevos_shows_them() {
        let v = |n: i32| Typed::signed(n);
        assert_eq!(Format::Value.show(v(1234567)), "1,234,567");
        assert_eq!(Format::Value.show(v(-1234)), "-1,234");
        assert_eq!(Format::Score.show(v(1234)), "001234");
        assert_eq!(Format::Frames.show(v(3600)), "1:00.00");
        assert_eq!(Format::Seconds.show(v(3725)), "1h02:05");
        assert_eq!(Format::Centisecs.show(v(12345)), "2:03.45");
        assert_eq!(Format::Minutes.show(v(125)), "2h05");
        assert_eq!(Format::Fixed(2).show(v(-1234)), "-12.34");
        assert_eq!(Format::Float(1).show(Typed::float(2.25)), "2.2");
        assert_eq!(Format::Thousands.show(v(12)), "12,000");
        assert_eq!(Format::parse("FLOAT3"), Format::Float(3));
        assert_eq!(Format::parse("FIXED4"), Format::Value);
        assert_eq!(Format::parse("TIME"), Format::Frames);
    }
}
