//! The router's name server: it answers the questions for IPv4 addresses
//! (type A) with what the host's resolver finds, and everything else with
//! no answer, which is all a 1990s program asks for.

use std::net::Ipv4Addr;

/// Type A, class IN.
const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;
/// How long answers may be kept.
const TTL: u32 = 60;

/// A question from a query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub id: u16,
    /// The name, dotted, without the final dot.
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
    /// The question as it came, for the answer to repeat.
    pub question: Vec<u8>,
    /// Whether the client asked for recursion.
    pub recursion: bool,
}

impl Query {
    /// Whether the host's resolver is asked.
    pub fn wants_address(&self) -> bool {
        self.qtype == TYPE_A && self.qclass == CLASS_IN
    }
}

/// The query in `message`, if it is one with one question.
pub fn parse(message: &[u8]) -> Option<Query> {
    if message.len() < 12 {
        return None;
    }
    let flags = u16::from_be_bytes([message[2], message[3]]);
    // A query (QR 0), a standard one (opcode 0), with one question.
    if flags & 0xF800 != 0 || u16::from_be_bytes([message[4], message[5]]) != 1 {
        return None;
    }
    let mut at = 12;
    let mut labels = Vec::new();
    loop {
        let len = *message.get(at)? as usize;
        at += 1;
        if len == 0 {
            break;
        }
        // No compression in a question, and 63 bytes a label at most.
        if len > 63 {
            return None;
        }
        let label = message.get(at..at + len)?;
        labels.push(String::from_utf8_lossy(label).into_owned());
        at += len;
    }
    let qtype = u16::from_be_bytes(message.get(at..at + 2)?.try_into().ok()?);
    let qclass = u16::from_be_bytes(message.get(at + 2..at + 4)?.try_into().ok()?);
    Some(Query {
        id: u16::from_be_bytes([message[0], message[1]]),
        name: labels.join("."),
        qtype,
        qclass,
        question: message[12..at + 4].to_vec(),
        recursion: flags & 0x0100 != 0,
    })
}

/// The answer to `query`: `addresses`, or with none a "no such name"
/// (`found` false) or an empty answer.
pub fn answer(query: &Query, addresses: &[Ipv4Addr], found: bool) -> Vec<u8> {
    let rcode = if found { 0 } else { 3 };
    // A response, recursion desired as asked and available.
    let flags = 0x8080 | if query.recursion { 0x0100 } else { 0 } | rcode;
    let mut out = Vec::with_capacity(12 + query.question.len() + 16 * addresses.len());
    out.extend_from_slice(&query.id.to_be_bytes());
    out.extend_from_slice(&(flags as u16).to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(addresses.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&query.question);
    for address in addresses {
        out.extend_from_slice(&[0xC0, 12]); // the name, at the question
        out.extend_from_slice(&TYPE_A.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out.extend_from_slice(&TTL.to_be_bytes());
        out.extend_from_slice(&4u16.to_be_bytes());
        out.extend_from_slice(&address.octets());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(name: &str, qtype: u16) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in name.split('.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&1u16.to_be_bytes());
        q
    }

    #[test]
    fn answers_address_questions() {
        let q = parse(&query("www.example.com", TYPE_A)).unwrap();
        assert_eq!((q.id, q.name.as_str(), q.wants_address(), q.recursion), (0x1234, "www.example.com", true, true));
        let a = answer(&q, &[Ipv4Addr::new(93, 184, 216, 34), Ipv4Addr::new(1, 2, 3, 4)], true);
        assert_eq!(&a[0..2], &[0x12, 0x34]);
        assert_eq!(u16::from_be_bytes([a[2], a[3]]) & 0x800F, 0x8000, "a response, no error");
        assert_eq!(u16::from_be_bytes([a[6], a[7]]), 2, "two answers");
        assert_eq!(&a[12..12 + q.question.len()], &q.question[..]);
        assert_eq!(&a[a.len() - 4..], &[1, 2, 3, 4]);
        let none = answer(&q, &[], false);
        assert_eq!(none[3] & 0x0F, 3, "no such name");
        // Other questions are asked, but not of the resolver.
        assert!(!parse(&query("example.com", 28)).unwrap().wants_address());
        // Not a query.
        let mut response = query("example.com", TYPE_A);
        response[2] |= 0x80;
        assert_eq!(parse(&response), None);
        assert_eq!(parse(&[0; 5]), None);
    }
}
