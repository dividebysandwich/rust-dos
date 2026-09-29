//! Leaderboards: `STA:…::CAN:…::SUB:…::VAL:…` (and `PRO:` for the value
//! shown while one runs). An attempt starts when its start trigger holds,
//! and ends with the value submitted, or cancelled.

use super::memref::Memrefs;
use super::parse::{Cursor, Error, Parse};
use super::trigger::Trigger;
use super::value::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LboardState {
    Inactive,
    /// It can't start until its start trigger has been false.
    Waiting,
    Active,
    Started,
    Canceled,
    Triggered,
    Disabled,
}

#[derive(Clone, Debug)]
pub struct Lboard {
    pub start: Trigger,
    pub submit: Trigger,
    pub cancel: Trigger,
    pub value: Value,
    pub progress: Option<Value>,
    pub state: LboardState,
}

impl Lboard {
    pub fn parse(definition: &str, parse: &mut Parse) -> Result<Lboard, Error> {
        let mut s = Cursor::new(definition);
        let (mut start, mut cancel, mut submit, mut value, mut progress) =
            (None, None, None, None, None);
        loop {
            let tag: Vec<u8> = (0..3).map(|i| s.at(i).to_ascii_lowercase()).collect();
            let field = if s.at(3) == b':' { tag.as_slice() } else { b"" };
            match field {
                b"sta" | b"can" | b"sub" => {
                    let (slot, duplicated) = match field {
                        b"sta" => (&mut start, Error::DuplicatedStart),
                        b"can" => (&mut cancel, Error::DuplicatedCancel),
                        _ => (&mut submit, Error::DuplicatedSubmit),
                    };
                    if slot.is_some() {
                        return Err(duplicated);
                    }
                    s.skip(4);
                    if !s.at_end() && s.at(0) != b':' {
                        *slot = Some(Trigger::parse(&mut s, parse)?);
                    }
                }
                b"val" | b"pro" => {
                    let (slot, duplicated) = if field == b"val" {
                        (&mut value, Error::DuplicatedValue)
                    } else {
                        (&mut progress, Error::DuplicatedProgress)
                    };
                    if slot.is_some() {
                        return Err(duplicated);
                    }
                    s.skip(4);
                    if !s.at_end() && s.at(0) != b':' {
                        *slot = Some(Value::parse(&mut s, parse)?);
                    }
                }
                _ => {}
            }
            if s.at_end() || s.at(0) == b'"' {
                break;
            }
            if s.at(0) != b':' || s.at(1) != b':' {
                return Err(Error::InvalidLboardField);
            }
            s.skip(2);
        }
        Ok(Lboard {
            start: start.ok_or(Error::MissingStart)?,
            cancel: cancel.ok_or(Error::MissingCancel)?,
            submit: submit.ok_or(Error::MissingSubmit)?,
            value: value.ok_or(Error::MissingValue)?,
            progress,
            state: LboardState::Waiting,
        })
    }

    /// Check it for this frame: its state, and the value it has (0 when
    /// no attempt runs).
    pub fn evaluate(&mut self, memrefs: &Memrefs) -> (LboardState, i32) {
        if matches!(self.state, LboardState::Inactive | LboardState::Disabled) {
            return (LboardState::Inactive, 0);
        }
        // Every frame, so hit counts work.
        let start_ok = self.start.test(memrefs);
        let cancel_ok = self.cancel.test(memrefs);
        let submit_ok = self.submit.test(memrefs);
        match self.state {
            LboardState::Waiting | LboardState::Triggered | LboardState::Canceled => {
                if start_ok {
                    return (LboardState::Inactive, 0);
                }
                self.state = LboardState::Active;
            }
            LboardState::Active => {
                if start_ok && !cancel_ok {
                    if submit_ok {
                        // Started and done in one frame: just submit.
                        self.state = LboardState::Triggered;
                    } else if !self.start.is_empty() {
                        self.state = LboardState::Started;
                    }
                    if let Some(progress) = &mut self.progress {
                        progress.reset();
                    }
                    self.value.reset();
                }
            }
            LboardState::Started => {
                if cancel_ok {
                    self.state = LboardState::Canceled;
                } else if submit_ok {
                    self.state = LboardState::Triggered;
                }
            }
            _ => {}
        }
        let value = match self.state {
            LboardState::Started => match &mut self.progress {
                Some(progress) => progress.evaluate(memrefs),
                None => self.value.evaluate(memrefs),
            },
            LboardState::Triggered => self.value.evaluate(memrefs),
            _ => 0,
        };
        (self.state, value)
    }

    pub fn reset(&mut self) {
        self.state = LboardState::Waiting;
        self.start.reset();
        self.submit.reset();
        self.cancel.reset();
        if let Some(progress) = &mut self.progress {
            progress.reset();
        }
        self.value.reset();
    }
}
