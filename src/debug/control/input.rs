//! Actor-owned input claims and queue ownership; independent of wire transports.
#![deny(clippy::all)]

use super::{LowInput, Shared};
use rust_dos::{
    control::{HoldHandle, HoldState, KeyChord, OperationId},
    keyboard::PcKey,
};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) const HOLD_LIMIT: usize = 8;
pub(super) const EVENT_LIMIT: usize = 28_673;
pub(super) const LEASE_LIMIT: Duration = Duration::from_secs(30);
const JOB_ACCEPTED: u8 = 1;
const JOB_COMPLETED: u8 = 2;

pub struct Job {
    pub(super) _permit: tokio::sync::OwnedSemaphorePermit,
    pub(super) _busy: super::control::Busy,
    pub(super) state: std::sync::atomic::AtomicU8,
}
impl Job {
    pub(super) fn accepted(&self) {
        self.state
            .store(JOB_ACCEPTED, std::sync::atomic::Ordering::Relaxed);
        self._busy.shared().metrics.accepted();
    }
    fn completed(&self) {
        self.state
            .store(JOB_COMPLETED, std::sync::atomic::Ordering::Relaxed);
        self._busy.shared().metrics.completed();
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        if self.state.load(std::sync::atomic::Ordering::Relaxed) == JOB_ACCEPTED {
            self._busy.shared().metrics.failed();
        }
    }
}

pub(super) struct Hold {
    pub button: Option<usize>,
    pub button_delivered: bool,
    pub handle: HoldHandle,
    pub keys: Vec<PcKey>,
    pub delivered: Vec<PcKey>,
    pub deadline: Instant,
    pub state: HoldState,
}

pub(super) struct Span {
    pub button: Option<usize>,
    pub button_delivered: bool,
    pub picture: Option<rust_dos::control::MousePicture>,
    pub req: u64,
    pub remaining: usize,
    pub _job: Option<Arc<Job>>,
    pub delivered: Vec<PcKey>,
    pub borrowed: Vec<HoldHandle>,
}

pub(super) struct EndedHold {
    handle: HoldHandle,
    state: HoldState,
    mouse: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct PictureMode {
    pub mode: rust_dos::video::VideoMode,
    pub vbe_mode: Option<u16>,
    pub width: u32,
    pub height: u32,
}

pub(super) struct Inputs {
    pub picture: Option<PictureMode>,
    pub mode_epoch: u64,
    pub generation: u64,
    pub power_ons: Option<u32>,
    pub holds: Vec<Hold>,
    pub ended: VecDeque<EndedHold>,
    pub spans: VecDeque<Span>,
    pub wait_owner: Option<u64>,
    pub controls: VecDeque<(u64, HoldHandle)>,
}

impl Default for Inputs {
    fn default() -> Self {
        Self {
            picture: None,
            mode_epoch: 0,
            generation: 1,
            power_ons: None,
            holds: Vec::new(),
            ended: VecDeque::new(),
            spans: VecDeque::new(),
            wait_owner: None,
            controls: VecDeque::new(),
        }
    }
}

impl Inputs {
    pub fn delivered_button(&mut self, button: usize, down: bool) {
        let Some(span) = self.spans.front_mut() else {
            return;
        };
        if span.button != Some(button) {
            return;
        }
        span.button_delivered = down;
        if let Some(hold) = self
            .holds
            .iter_mut()
            .find(|hold| hold.handle.operation.request == span.req)
            && hold.button == Some(button)
        {
            hold.button_delivered = down;
            if down {
                hold.state = HoldState::Delivered;
            }
        }
    }
    pub fn handle(&self, shared: &Shared, req: u64) -> HoldHandle {
        HoldHandle {
            operation: OperationId {
                instance: shared.instance_hex(),
                request: req,
            },
            generation: self.generation,
        }
    }

    pub fn chord_keys(chord: &KeyChord) -> Vec<PcKey> {
        let mut keys = chord.modifiers.clone();
        if !keys.contains(&chord.key) {
            keys.push(chord.key);
        }
        keys
    }

    pub fn owned(&self, key: PcKey) -> bool {
        self.holds.iter().any(|hold| hold.keys.contains(&key))
    }

    pub fn index(&self, handle: &HoldHandle) -> Option<usize> {
        self.holds.iter().position(|hold| hold.handle == *handle)
    }

    pub fn ended_state(&self, handle: &HoldHandle) -> Option<HoldState> {
        self.ended
            .iter()
            .find(|ended| ended.handle == *handle)
            .map(|ended| ended.state)
    }

    fn ended_mouse(&self, handle: &HoldHandle) -> Option<bool> {
        self.ended
            .iter()
            .find(|ended| ended.handle == *handle)
            .map(|ended| ended.mouse)
    }

    pub fn retire(&mut self, index: usize, state: HoldState) -> Hold {
        let mut hold = self.holds.remove(index);
        hold.state = state;
        if self.ended.len() == super::journal::OPS_CAPACITY {
            self.ended.pop_front();
        }
        self.ended.push_back(EndedHold {
            handle: hold.handle.clone(),
            state,
            mouse: hold.button.is_some(),
        });
        hold
    }

    pub fn note_key(&mut self, req: u64, key: PcKey) {
        if let Some(hold) = self
            .holds
            .iter_mut()
            .find(|hold| hold.handle.operation.request == req)
        {
            if hold.keys.contains(&key) && !hold.delivered.contains(&key) {
                hold.delivered.push(key);
            }
            if hold.delivered.len() == hold.keys.len() {
                hold.state = HoldState::Delivered;
            }
        }
    }

    pub fn owner(&self) -> Option<u64> {
        self.spans.front().map(|span| span.req)
    }

    pub fn consumed(&mut self) {
        if let Some(span) = self.spans.front_mut() {
            span.remaining -= 1;
            if span.remaining == 0 {
                if let Some(job) = &span._job {
                    job.completed();
                }
                self.spans.pop_front();
            }
        }
    }

    fn complete_and_remove(&mut self, queue: &mut VecDeque<LowInput>, req: u64) -> bool {
        if let Some(job) = self
            .spans
            .iter()
            .find(|span| span.req == req)
            .and_then(|span| span._job.as_ref())
        {
            job.completed();
        }
        self.remove(queue, req)
    }

    pub fn remove(&mut self, queue: &mut VecDeque<LowInput>, req: u64) -> bool {
        let mut offset = 0;
        for index in 0..self.spans.len() {
            let span = &self.spans[index];
            if span.req == req {
                queue.drain(offset..offset + span.remaining);
                self.spans.remove(index);
                return true;
            }
            offset += span.remaining;
        }
        false
    }
}

pub(super) fn token(handle: &HoldHandle) -> String {
    format!(
        "{}.{:016x}.{:016x}",
        handle.operation.instance, handle.generation, handle.operation.request
    )
}

pub(super) fn hold_value(hold: &Hold, now: Instant) -> serde_json::Value {
    serde_json::json!({"handle":token(&hold.handle),"state":match hold.state {
        HoldState::Reserved=>"reserved", HoldState::Delivered=>"delivered", HoldState::Released=>"released", HoldState::Expired=>"expired", HoldState::Reset=>"reset"
    },"remaining_ms":hold.deadline.saturating_duration_since(now).as_millis() as u64})
}

pub(super) fn parse_token(value: &str) -> Option<HoldHandle> {
    if value.len() != 50 {
        return None;
    }
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.len() != 16
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    {
        return None;
    }
    Some(HoldHandle {
        operation: OperationId {
            instance: parts[0].to_owned(),
            request: u64::from_str_radix(parts[2], 16).ok()?,
        },
        generation: u64::from_str_radix(parts[1], 16).ok()?,
    })
}

impl super::DebugHub {
    pub(super) fn native_remove(&mut self, cpu: &mut super::Cpu, req: u64) {
        if let Some(span) = self.owned_input.spans.iter().find(|span| span.req == req)
            && span.button_delivered
            && let Some(button) = span.button
        {
            cpu.bus.mouse.button_up(button);
        }
        let removed = self.owned_input.remove(&mut self.input, req);
        if self.owned_input.wait_owner == Some(req) {
            self.input_wait_until = None;
            self.input_wait_ns = None;
            self.owned_input.wait_owner = None;
        }
        if removed {
            self.append_then(
                cpu,
                super::Kind::InputCleared,
                Some(req),
                serde_json::json!({"cancelled":[req]}),
                |journal, seq| {
                    journal.end_op(req, super::OpState::Cancelled, Some(seq));
                },
            );
        }
    }

    pub(super) fn native_retire(&mut self, cpu: &mut super::Cpu, index: usize, state: HoldState) {
        let complete = self.owned_input.holds[index].state == HoldState::Delivered;
        let hold = self.owned_input.retire(index, state);
        if complete
            && self
                .owned_input
                .complete_and_remove(&mut self.input, hold.handle.operation.request)
        {
            self.input_delivered(cpu, hold.handle.operation.request);
        }
        self.native_remove(cpu, hold.handle.operation.request);
        for key in hold.delivered.iter().rev() {
            self.key_up(cpu, *key);
        }
        if hold.button_delivered
            && let Some(button) = hold.button
        {
            cpu.bus.mouse.button_up(button);
        }
    }

    pub(super) fn native_expire(&mut self, cpu: &mut super::Cpu) {
        let now = Instant::now();
        for index in (0..self.owned_input.holds.len()).rev() {
            if self.owned_input.holds[index].deadline <= now {
                self.native_retire(cpu, index, HoldState::Expired);
            }
        }
    }

    pub(super) fn native_input(
        &mut self,
        cpu: &mut super::Cpu,
        input: rust_dos::control::GuestInput,
        job: Option<Arc<Job>>,
    ) -> super::Reply {
        use super::{InputEvent, KeyAction, Kind, OpKind, Reply};
        use rust_dos::control::GuestInput;
        self.native_expire(cpu);
        let req = self.cur.as_ref().map_or(0, |ctx| ctx.req);
        let Some(shared) = self.shared.clone() else {
            return Reply::Error(503, "input unavailable".into());
        };
        let now = Instant::now();
        let control = match &input {
            GuestInput::Up { handle } => Some((handle, None, false)),
            GuestInput::Renew { handle, lease } => Some((handle, Some(*lease), false)),
            GuestInput::Mouse(rust_dos::control::MouseInput::Up { handle }) => {
                Some((handle, None, true))
            }
            GuestInput::Mouse(rust_dos::control::MouseInput::Renew { handle, lease }) => {
                Some((handle, Some(*lease), true))
            }
            _ => None,
        };
        if let Some((handle, renewal, mouse)) = control {
            if handle.operation.instance != shared.instance_hex() {
                return Reply::Error(409, "stale input instance".into());
            }
            if handle.generation != self.owned_input.generation {
                return Reply::Error(409, "hold reset".into());
            }
            if let Some(index) = self.owned_input.index(handle) {
                if self.owned_input.holds[index].button.is_some() != mouse {
                    return Reply::bad("hold belongs to another input device");
                }
                match renewal {
                    Some(lease) => {
                        if lease.is_zero() || lease > LEASE_LIMIT {
                            return Reply::bad("invalid hold lease");
                        }
                        self.owned_input.holds[index].deadline = now + lease;
                        let hold = hold_value(&self.owned_input.holds[index], now);
                        return self.native_control_done(cpu, req, handle, hold);
                    }
                    _ => {
                        self.native_retire(cpu, index, HoldState::Released);
                        return self.native_control_done(cpu, req, handle,
                            serde_json::json!({"handle":token(handle),"state":"released","remaining_ms":0}),
                        );
                    }
                }
            }
            if self
                .owned_input
                .ended_mouse(handle)
                .is_some_and(|ended_mouse| ended_mouse != mouse)
            {
                return Reply::bad("handle belongs to a different input device");
            }
            if renewal.is_none()
                && self.owned_input.ended_state(handle) == Some(HoldState::Released)
            {
                return self.native_control_done(
                    cpu,
                    req,
                    handle,
                    serde_json::json!({"handle":token(handle),"state":"released","remaining_ms":0}),
                );
            }
            let message = match self.owned_input.ended_state(handle) {
                Some(HoldState::Expired) => "hold expired",
                Some(HoldState::Released) => "hold released",
                Some(HoldState::Reset) => "hold reset",
                _ => "unknown hold",
            };
            return Reply::Error(409, message.into());
        }
        if self.divert || self.rewinding {
            return Reply::Error(409, "guest input target changed".into());
        }
        if let Err(reply) = self.check_input_capacity() {
            return reply;
        }
        if let GuestInput::Mouse(mouse) = input {
            return self.native_mouse_input(cpu, mouse, job);
        }
        let canonical = |key: PcKey| {
            super::keys::names()
                .into_iter()
                .find(|name| {
                    super::keys::lookup(name).is_some_and(|known| {
                        known == key
                            && known.ascii == key.ascii
                            && known.shifted == key.shifted
                            && known.modifier == key.modifier
                    })
                })
                .map(str::to_owned)
        };
        let persistent = matches!(input, GuestInput::Down { .. });
        let (event, chord, lease) = match input {
            GuestInput::Press { chord, duration }
            | GuestInput::Down {
                chord,
                lease: duration,
            } => {
                if chord.modifiers.len() > 4
                    || chord.modifiers.iter().enumerate().any(|(index, modifier)| {
                        modifier.modifier == 0
                            || *modifier == chord.key
                            || chord.modifiers[..index].contains(modifier)
                    })
                {
                    return Reply::bad("invalid modifier set");
                }
                if duration.is_zero()
                    || duration
                        > if persistent {
                            LEASE_LIMIT
                        } else {
                            Duration::from_secs(5)
                        }
                {
                    return Reply::bad("invalid input duration");
                }
                let Some(key) = canonical(chord.key) else {
                    return Reply::bad("unknown key");
                };
                let Some(mods) = chord
                    .modifiers
                    .iter()
                    .map(|key| canonical(*key))
                    .collect::<Option<Vec<_>>>()
                else {
                    return Reply::bad("unknown modifier");
                };
                let event = InputEvent::Key {
                    key: Some(key),
                    scancode: None,
                    ascii: None,
                    action: if persistent {
                        KeyAction::Down
                    } else {
                        KeyAction::Press
                    },
                    mods,
                    hold_ms: Some(duration.as_millis() as u64),
                };
                (event, Some(chord), persistent.then_some(duration))
            }
            GuestInput::Type { text, delay } => {
                if delay > Duration::from_secs(5) {
                    return Reply::bad("invalid typing delay");
                }
                if text.len() > 4096 {
                    return Reply::bad("text exceeds input limit");
                }
                (
                    InputEvent::Type {
                        text,
                        delay_ms: (!delay.is_zero()).then_some(delay.as_millis() as u64),
                    },
                    None,
                    None,
                )
            }
            _ => unreachable!(),
        };
        let mut low = Vec::with_capacity(EVENT_LIMIT);
        if let Err(error) = super::expand_input(&event, cpu.bus.typing_layout(), &mut low) {
            return Reply::bad(error);
        }
        if low.len() + 1 > EVENT_LIMIT {
            return Reply::bad("expanded input exceeds event limit");
        }
        let wanted_mods = chord.as_ref().map_or(0, |chord| {
            chord
                .modifiers
                .iter()
                .fold(chord.key.modifier, |bits, key| bits | key.modifier)
        });
        let mut projected_keys = self.remote_held.clone();
        for event in &self.input {
            match event {
                LowInput::KeyDown { key, .. } => {
                    if !projected_keys.contains(key) {
                        projected_keys.push(*key);
                    }
                }
                LowInput::KeyUp { key } => {
                    projected_keys.retain(|held| held != key);
                }
                _ => {}
            }
        }
        let projected_mods = projected_keys
            .iter()
            .fold(0, |mods, key| mods | key.modifier);
        if projected_mods & !wanted_mods != 0
            || (self
                .owned_input
                .holds
                .iter()
                .any(|hold| hold.button.is_none())
                && wanted_mods != projected_mods
                && chord.as_ref().is_some_and(|chord| chord.key.modifier != 0))
        {
            return Reply::Error(409, "effective modifier conflict".into());
        }
        if low.iter().any(|event| matches!(event, LowInput::KeyDown { key, .. }
            if key.modifier != 0 && projected_mods & key.modifier != 0 && !projected_keys.contains(key)))
        {
            return Reply::Error(409, "effective modifier conflict".into());
        }
        if let Some(chord) = &chord
            && (self.owned_input.owned(chord.key) || projected_keys.contains(&chord.key))
        {
            return Reply::Error(409, "key reserved".into());
        }
        if low.iter().any(|event| {
            matches!(event, LowInput::KeyDown { key, .. }
            if key.modifier == 0 && projected_keys.contains(key))
        }) {
            return Reply::Error(409, "key reserved".into());
        }
        if let Some(lease) = lease {
            if self.owned_input.holds.len() >= HOLD_LIMIT {
                return Reply::Error(429, "hold capacity".into());
            }
            let keys = Inputs::chord_keys(chord.as_ref().unwrap());
            if keys
                .iter()
                .any(|key| self.owned_input.owned(*key) || projected_keys.contains(key))
            {
                return Reply::Error(409, "key or modifier reserved".into());
            }
            let handle = self.owned_input.handle(&shared, req);
            self.owned_input.holds.push(Hold {
                button: None,
                button_delivered: false,
                handle,
                keys,
                delivered: Vec::new(),
                deadline: now + lease,
                state: HoldState::Reserved,
            });
        } else {
            low.retain(|event| !matches!(event, LowInput::KeyDown { key, .. } | LowInput::KeyUp { key } if key.modifier != 0 && projected_keys.contains(key)));
        }
        let borrowed = if lease.is_none() {
            self.owned_input
                .holds
                .iter()
                .filter(|hold| hold.keys.iter().any(|key| key.modifier & wanted_mods != 0))
                .map(|hold| hold.handle.clone())
                .collect()
        } else {
            Vec::new()
        };
        if let Some(job) = &job {
            job.accepted();
        }
        self.owned_input.spans.push_back(Span {
            button: None,
            button_delivered: false,
            picture: None,
            req,
            remaining: low.len() + 1,
            _job: job,
            delivered: Vec::new(),
            borrowed,
        });
        self.input.extend(low);
        self.input.push_back(LowInput::End { req });
        let tag = self.cur.as_ref().and_then(|ctx| ctx.tag.clone());
        self.append_then(
            cpu,
            Kind::InputQueued,
            Some(req),
            serde_json::json!({"events":1,"wait":false}),
            |journal, seq| journal.begin_op(req, OpKind::Input, seq, tag),
        );
        if let Some(ctx) = self.cur.as_mut() {
            ctx.op = Some(req);
        }
        let mut result = serde_json::json!({"ok":true,"op":req,"held":self.deterministic.is_some() && self.paused});
        if let Some(hold) = self
            .owned_input
            .holds
            .iter()
            .find(|hold| hold.handle.operation.request == req)
        {
            result["hold"] = hold_value(hold, now);
        }
        super::Reply::Json(result)
    }
}

impl super::DebugHub {
    pub(super) fn native_legacy_conflict(&self, events: &[LowInput]) -> bool {
        let mouse_owned = self
            .owned_input
            .holds
            .iter()
            .any(|hold| hold.button.is_some())
            || self
                .owned_input
                .spans
                .iter()
                .any(|span| span.picture.is_some());
        if events.iter().any(|event| match event {
            LowInput::MouseTo { .. } | LowInput::MouseRel { .. } => mouse_owned,
            LowInput::Button { idx, .. } => {
                self.owned_input
                    .holds
                    .iter()
                    .any(|hold| hold.button == Some(*idx))
                    || self
                        .owned_input
                        .spans
                        .iter()
                        .any(|span| span.button == Some(*idx))
            }
            _ => false,
        }) {
            return true;
        }
        if self
            .owned_input
            .holds
            .iter()
            .all(|hold| hold.button.is_some())
        {
            return false;
        }
        let mut projected = self.remote_mods;
        for event in &self.input {
            match event {
                LowInput::KeyDown { key, .. } => projected |= key.modifier,
                LowInput::KeyUp { key } => projected &= !key.modifier,
                _ => {}
            }
        }
        let mut wanted = 0;
        for event in events {
            match event {
                LowInput::KeyDown { key, .. } => {
                    if self.owned_input.owned(*key) || key.modifier != 0 {
                        return true;
                    }
                    if projected != wanted {
                        return true;
                    }
                    wanted |= key.modifier;
                    projected |= key.modifier;
                }
                LowInput::KeyUp { key } => {
                    if self.owned_input.owned(*key) || key.modifier != 0 {
                        return true;
                    }
                    wanted &= !key.modifier;
                    projected &= !key.modifier;
                }
                LowInput::Char(_) if projected != 0 => return true,
                _ => {}
            }
        }
        false
    }
}

impl super::DebugHub {
    fn native_control_done(
        &mut self,
        cpu: &super::Cpu,
        req: u64,
        handle: &HoldHandle,
        hold: serde_json::Value,
    ) -> super::Reply {
        use super::{Kind, OpKind, OpState};
        let tag = self.cur.as_ref().and_then(|ctx| ctx.tag.clone());
        self.append_then(
            cpu,
            Kind::InputQueued,
            Some(req),
            serde_json::json!({"events":0,"wait":false}),
            |journal, seq| journal.begin_op(req, OpKind::Input, seq, tag),
        );
        self.append_then(
            cpu,
            Kind::InputDelivered,
            Some(req),
            serde_json::json!({}),
            |journal, seq| {
                journal.end_op(req, OpState::Done, Some(seq));
            },
        );
        if let Some(ctx) = self.cur.as_mut() {
            ctx.op = Some(req);
        }
        if self.owned_input.controls.len() == super::journal::OPS_CAPACITY {
            self.owned_input.controls.pop_front();
        }
        self.owned_input.controls.push_back((req, handle.clone()));
        super::Reply::Json(serde_json::json!({"ok":true,"op":req,"hold":hold}))
    }

    pub(super) fn native_operation(
        &mut self,
        cpu: &mut super::Cpu,
        id: OperationId,
    ) -> super::Reply {
        use super::{OpKind, OpState, Reply};
        self.native_expire(cpu);
        let Some(shared) = self.shared.as_ref() else {
            return Reply::Error(503, "input unavailable".into());
        };
        if id.instance != shared.instance_hex() {
            return Reply::Error(409, "stale operation instance".into());
        }
        let operation = self
            .with_journal(|journal| match journal.op(id.request) {
                super::journal::OpLookup::Found(op) if op.kind == OpKind::Input => Ok(op.state),
                super::journal::OpLookup::Gone => Err(410),
                _ => Err(404),
            })
            .unwrap_or(Err(503));
        let state = match operation {
            Ok(state) => state,
            Err(code) => return Reply::Error(code, "operation unavailable".into()),
        };
        let mut value = serde_json::json!({"op":id.request,"delivery":match state { OpState::Pending=>"pending",OpState::Done=>"delivered",OpState::Cancelled=>"cancelled",_=>"failed" }});
        let acquisition = self
            .owned_input
            .controls
            .iter()
            .find(|(req, _)| *req == id.request)
            .map_or(id.request, |(_, handle)| handle.operation.request);
        if let Some(hold) = self
            .owned_input
            .holds
            .iter()
            .find(|hold| hold.handle.operation.request == acquisition)
        {
            value["hold"] = hold_value(hold, Instant::now());
        } else if let Some(ended) = self
            .owned_input
            .ended
            .iter()
            .find(|ended| ended.handle.operation.request == acquisition)
        {
            value["hold"] = serde_json::json!({"handle":token(&ended.handle),"state":match ended.state {HoldState::Released=>"released",HoldState::Expired=>"expired",HoldState::Reset=>"reset",_=>"unknown"},"remaining_ms":0});
        }
        Reply::Json(value)
    }
}

impl super::DebugHub {
    pub(super) fn native_guard_target(&mut self, cpu: &mut super::Cpu) {
        if self
            .owned_input
            .power_ons
            .replace(cpu.bus.power_ons)
            .is_some_and(|previous| previous != cpu.bus.power_ons)
        {
            self.invalidate_owned_after_load(cpu);
            self.forget_keys_after_load();
        }
        if (self.divert || self.rewinding)
            && (!self.owned_input.holds.is_empty()
                || self
                    .owned_input
                    .spans
                    .iter()
                    .any(|span| span._job.is_some()))
        {
            self.release_owned_input(cpu);
        }
    }

    pub(super) fn native_cancel_expired_borrow(&mut self, cpu: &mut super::Cpu) -> bool {
        let Some(span) = self.owned_input.spans.front() else {
            return false;
        };
        let now = Instant::now();
        if !span.borrowed.iter().any(|handle| {
            self.owned_input
                .index(handle)
                .is_none_or(|index| self.owned_input.holds[index].deadline <= now)
        }) {
            return false;
        }
        let req = span.req;
        let delivered = span.delivered.clone();
        self.native_remove(cpu, req);
        for key in delivered.iter().rev() {
            self.key_up(cpu, *key);
        }
        true
    }

    /// Detach old ownership after replacement without sending releases to the restored guest.
    pub(crate) fn invalidate_owned_after_load(&mut self, cpu: &super::Cpu) {
        let cancelled = self.native_invalidate_restored(cpu);
        if cancelled.is_empty() {
            return;
        }
        self.append_then(
            cpu,
            super::Kind::InputCleared,
            None,
            serde_json::json!({"cancelled":cancelled}),
            |journal, seq| {
                for req in cancelled {
                    journal.end_op(req, super::OpState::Cancelled, Some(seq));
                }
            },
        );
    }

    pub(super) fn native_invalidate_restored(&mut self, cpu: &super::Cpu) -> Vec<u64> {
        self.owned_input.picture = None;
        self.owned_input.mode_epoch = self.owned_input.mode_epoch.saturating_add(1);
        let mut keys = self
            .owned_input
            .holds
            .iter()
            .flat_map(|hold| hold.delivered.iter().copied())
            .collect::<Vec<_>>();
        keys.extend(
            self.owned_input
                .spans
                .iter()
                .filter(|span| span._job.is_some())
                .flat_map(|span| span.delivered.iter().copied()),
        );
        self.remote_held.retain(|key| !keys.contains(key));
        self.remote_mods &= !keys.iter().fold(0, |bits, key| bits | key.modifier);
        let requests = self
            .owned_input
            .spans
            .iter()
            .filter(|span| span._job.is_some())
            .map(|span| span.req)
            .collect::<Vec<_>>();
        let mut cancelled = Vec::new();
        for req in requests {
            let complete = self.owned_input.holds.iter().any(|hold| {
                hold.handle.operation.request == req && hold.state == HoldState::Delivered
            });
            if complete {
                self.owned_input.complete_and_remove(&mut self.input, req);
                self.input_delivered(cpu, req);
            } else {
                self.owned_input.remove(&mut self.input, req);
                cancelled.push(req);
            }
            if self.owned_input.wait_owner == Some(req) {
                self.input_wait_until = None;
                self.input_wait_ns = None;
                self.owned_input.wait_owner = None;
            }
        }
        for index in (0..self.owned_input.holds.len()).rev() {
            self.owned_input.retire(index, HoldState::Reset);
        }
        self.owned_input.generation = self
            .owned_input
            .generation
            .checked_add(1)
            .expect("input generation exhausted");
        cancelled
    }

    pub fn release_owned_input(&mut self, cpu: &mut super::Cpu) {
        for index in (0..self.owned_input.holds.len()).rev() {
            self.native_retire(cpu, index, HoldState::Reset);
        }
        let keys = self
            .owned_input
            .spans
            .iter()
            .filter(|span| span._job.is_some())
            .flat_map(|span| span.delivered.iter().copied())
            .collect::<Vec<_>>();
        let requests = self
            .owned_input
            .spans
            .iter()
            .filter(|span| span._job.is_some())
            .map(|span| span.req)
            .collect::<Vec<_>>();
        for req in requests {
            self.native_remove(cpu, req);
        }
        for key in keys.iter().rev() {
            self.key_up(cpu, *key);
        }
        self.owned_input.generation = self
            .owned_input
            .generation
            .checked_add(1)
            .expect("input generation exhausted");
    }
}
