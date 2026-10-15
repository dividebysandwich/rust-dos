//! Mouse expansion uses the actor's existing queue and coordinate conversion.
#![deny(clippy::all)]
use super::owned_input::{HOLD_LIMIT, Hold, Job, LEASE_LIMIT, PictureMode, Span};
use super::{Cpu, DebugHub, LowInput, Reply};
use rust_dos::control::{HoldState, MouseButton, MouseInput, MouseMotion, MousePicture};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) const PATH_LIMIT: usize = 256;
pub(super) const GESTURE_LIMIT: Duration = Duration::from_secs(5);

impl DebugHub {
    pub(super) fn native_sync_picture(&mut self, cpu: &Cpu) {
        let (width, height) = super::video::frame_size(&cpu.bus);
        let picture = PictureMode {
            mode: cpu.bus.video_mode,
            vbe_mode: cpu.bus.vbe.mode.map(|mode| mode.number),
            width,
            height,
        };
        if self.owned_input.picture != Some(picture) {
            self.owned_input.mode_epoch = self.owned_input.mode_epoch.saturating_add(1);
            self.owned_input.picture = Some(picture);
        }
    }

    fn mouse_picture_matches(&self, picture: &MousePicture) -> bool {
        self.shared
            .as_ref()
            .is_some_and(|shared| shared.instance_hex() == picture.instance)
            && picture.mode_epoch == self.owned_input.mode_epoch
            && self.owned_input.picture.is_some_and(|mode| {
                mode.width == picture.size.width && mode.height == picture.size.height
            })
    }

    pub(super) fn native_mouse_guard(&mut self, cpu: &mut Cpu) {
        let stale = self
            .owned_input
            .spans
            .iter()
            .filter(|span| {
                span.picture
                    .as_ref()
                    .is_some_and(|picture| !self.mouse_picture_matches(picture))
            })
            .map(|span| span.req)
            .collect::<Vec<_>>();
        for req in stale {
            if let Some(index) = self
                .owned_input
                .holds
                .iter()
                .position(|hold| hold.handle.operation.request == req)
            {
                self.native_retire(cpu, index, HoldState::Reset);
            } else {
                self.native_remove(cpu, req);
            }
        }
    }

    pub(super) fn native_mouse_input(
        &mut self,
        cpu: &mut Cpu,
        input: MouseInput,
        job: Option<Arc<Job>>,
    ) -> Reply {
        self.native_sync_picture(cpu);
        self.native_mouse_guard(cpu);
        let req = self.cur.as_ref().map_or(0, |ctx| ctx.req);
        let Some(shared) = self.shared.clone() else {
            return Reply::Error(503, "input unavailable".into());
        };
        let (picture, motion, button, lease, duration, path, owner) = match input {
            MouseInput::Move {
                picture,
                motion,
                handle,
            } => (picture, Some(motion), None, None, None, None, handle),
            MouseInput::Click {
                picture,
                motion,
                button,
                duration,
            } => (
                picture,
                motion,
                Some(button),
                None,
                Some(duration),
                None,
                None,
            ),
            MouseInput::Down {
                picture,
                motion,
                button,
                lease,
            } => (picture, motion, Some(button), Some(lease), None, None, None),
            MouseInput::Drag {
                picture,
                button,
                path,
            } => (picture, None, Some(button), None, None, Some(path), None),
            MouseInput::Up { .. } | MouseInput::Renew { .. } => {
                unreachable!("shared control handles up and renew")
            }
        };
        if !self.mouse_picture_matches(&picture) {
            return Reply::Error(409, "stale mouse picture; capture again".into());
        }
        let button = button.map(|button| match button {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
        });
        if lease.is_some_and(|lease| lease.is_zero() || lease > LEASE_LIMIT)
            || duration.is_some_and(|duration| duration.is_zero() || duration > GESTURE_LIMIT)
        {
            return Reply::bad("invalid mouse duration");
        }
        if let Some(owner) = &owner {
            let Some(index) = self.owned_input.index(owner) else {
                return Reply::Error(409, "unknown mouse hold".into());
            };
            if self.owned_input.holds[index].button.is_none() {
                return Reply::bad("handle is not a mouse hold");
            }
        }
        let moves = motion.is_some() || path.is_some();
        let foreign_hold = self.owned_input.holds.iter().any(|hold| {
            hold.button.is_some()
                && owner.as_ref() != Some(&hold.handle)
                && (moves || hold.button == button)
        });
        let foreign_span = self.owned_input.spans.iter().any(|span| {
            span.picture.is_some()
                && owner.as_ref().is_none_or(|owner| {
                    owner.operation.request != span.req && !span.borrowed.contains(owner)
                })
        });
        if foreign_hold || foreign_span {
            return Reply::Error(409, "mouse pointer or button reserved".into());
        }
        let mut projected_buttons = cpu.bus.mouse.buttons as u64;
        for event in &self.input {
            if let LowInput::Button { idx, down } = event {
                if *down {
                    projected_buttons |= 1 << idx;
                } else {
                    projected_buttons &= !(1 << idx);
                }
            }
        }
        let owned_buttons = self
            .owned_input
            .holds
            .iter()
            .filter_map(|hold| hold.button)
            .fold(0u64, |mask, button| mask | (1 << button));
        if (moves && projected_buttons & !owned_buttons != 0)
            || button.is_some_and(|button| projected_buttons & (1 << button) != 0)
        {
            return Reply::Error(409, "legacy mouse button reserved".into());
        }
        if lease.is_some() && self.owned_input.holds.len() >= HOLD_LIMIT {
            return Reply::Error(429, "hold capacity".into());
        }
        if path
            .as_ref()
            .is_some_and(|path| path.is_empty() || path.len() > PATH_LIMIT)
        {
            return Reply::bad("invalid mouse path length");
        }
        let capacity = path.as_ref().map_or(5, |path| path.len() * 2 + 3);
        let mut low = Vec::new();
        if low.try_reserve_exact(capacity).is_err() {
            return Reply::Error(503, "mouse input allocation unavailable".into());
        }
        let movement = |motion: MouseMotion| -> Result<LowInput, &'static str> {
            match motion {
                MouseMotion::Absolute { x, y }
                    if x >= 0
                        && y >= 0
                        && (x as u32) < picture.size.width
                        && (y as u32) < picture.size.height =>
                {
                    Ok(LowInput::MouseTo {
                        x,
                        y,
                        coords: super::Coords::Screen,
                    })
                }
                MouseMotion::Relative { dx, dy }
                    if dx.unsigned_abs() <= 32767 && dy.unsigned_abs() <= 32767 =>
                {
                    Ok(LowInput::MouseRel { dx, dy })
                }
                _ => Err("mouse coordinates outside accepted range"),
            }
        };
        if let Some(path) = path {
            let absolute = matches!(path[0].motion, MouseMotion::Absolute { .. });
            let mut total = Duration::ZERO;
            for (index, step) in path.iter().enumerate() {
                if matches!(step.motion, MouseMotion::Absolute { .. }) != absolute {
                    return Reply::bad("mixed mouse path coordinates");
                }
                let Some(sum) = total.checked_add(step.delay) else {
                    return Reply::bad("mouse gesture duration overflow");
                };
                total = sum;
                if total > GESTURE_LIMIT {
                    return Reply::bad("mouse gesture exceeds duration limit");
                }
                let event = match movement(step.motion) {
                    Ok(event) => event,
                    Err(error) => return Reply::bad(error),
                };
                low.push(event);
                if index == 0 {
                    low.push(LowInput::Button {
                        idx: button.expect("drag has a button"),
                        down: true,
                    });
                }
                if !step.delay.is_zero() {
                    low.push(LowInput::Wait(step.delay));
                }
            }
            low.push(LowInput::Button {
                idx: button.expect("drag has a button"),
                down: false,
            });
        } else {
            if let Some(motion) = motion {
                match movement(motion) {
                    Ok(event) => low.push(event),
                    Err(error) => return Reply::bad(error),
                }
            }
            if let Some(button) = button {
                low.push(LowInput::Button {
                    idx: button,
                    down: true,
                });
                if let Some(duration) = duration {
                    low.push(LowInput::Wait(duration));
                    low.push(LowInput::Button {
                        idx: button,
                        down: false,
                    });
                }
            }
        }
        if let Some(lease) = lease {
            self.owned_input.holds.push(Hold {
                handle: self.owned_input.handle(&shared, req),
                keys: Vec::new(),
                delivered: Vec::new(),
                button,
                button_delivered: false,
                deadline: Instant::now() + lease,
                state: HoldState::Reserved,
            });
        }
        let events = low.len();
        if let Some(job) = &job {
            job.accepted();
        }
        self.owned_input.spans.push_back(Span {
            req,
            remaining: events + 1,
            _job: job,
            delivered: Vec::new(),
            borrowed: owner.into_iter().collect(),
            button,
            button_delivered: false,
            picture: Some(picture),
        });
        self.input.extend(low);
        self.input.push_back(LowInput::End { req });
        let tag = self.cur.as_ref().and_then(|ctx| ctx.tag.clone());
        self.append_then(
            cpu,
            super::Kind::InputQueued,
            Some(req),
            serde_json::json!({"events":events,"wait":false}),
            |journal, seq| journal.begin_op(req, super::OpKind::Input, seq, tag),
        );
        if let Some(ctx) = self.cur.as_mut() {
            ctx.op = Some(req)
        }
        let mut result = serde_json::json!({"ok":true,"op":req,"held":self.deterministic.is_some()&&self.paused});
        if let Some(hold) = self
            .owned_input
            .holds
            .iter()
            .find(|hold| hold.handle.operation.request == req)
        {
            result["hold"] = super::owned_input::hold_value(hold, Instant::now());
        }
        Reply::Json(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_dos::control::PictureSize;

    #[test]
    fn disabled_actor_rejects_input_without_touching_the_guest() {
        let directory =
            std::env::temp_dir().join(format!("rust-dos-disabled-mouse-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut cpu = Cpu::new(directory.clone());
        let mut hub = DebugHub::disabled();
        // This is an unavailable-service check, not a captured-picture fixture.
        let picture = MousePicture {
            instance: String::new(),
            mode_epoch: 0,
            size: PictureSize {
                width: 0,
                height: 0,
            },
        };
        assert!(!hub.mouse_picture_matches(&picture));
        let before = (cpu.bus.mouse.x, cpu.bus.mouse.y, cpu.bus.mouse.buttons);
        let reply = hub.native_mouse_input(
            &mut cpu,
            MouseInput::Move {
                picture,
                motion: MouseMotion::Relative { dx: 1, dy: 1 },
                handle: None,
            },
            None,
        );
        assert!(matches!(reply, Reply::Error(503, _)));
        let keyboard = hub.native_input(
            &mut cpu,
            rust_dos::control::GuestInput::Type {
                text: "A".into(),
                delay: Duration::ZERO,
            },
            None,
        );
        assert!(matches!(keyboard, Reply::Error(503, _)));
        let operation = hub.native_operation(
            &mut cpu,
            rust_dos::control::OperationId {
                instance: String::new(),
                request: 1,
            },
        );
        assert!(matches!(operation, Reply::Error(503, _)));
        assert_eq!(
            (cpu.bus.mouse.x, cpu.bus.mouse.y, cpu.bus.mouse.buttons),
            before
        );
        assert!(hub.input.is_empty());
        assert!(hub.remote_held.is_empty());
        assert!(hub.owned_input.holds.is_empty());
        assert!(hub.owned_input.spans.is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
