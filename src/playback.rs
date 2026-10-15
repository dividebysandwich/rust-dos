//! Frontend-owned execution controls; transport adapters never touch the pacer.
use crate::{cpu::Cpu, timer::Pacer};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(crate) const MAX_FAST_FORWARD_MS: u64 = 5_000;

#[derive(Clone, Debug)]
// The optional MCP adapter constructs fast-forward actions; HTTP uses Speed.
#[cfg_attr(not(feature = "mcp"), allow(dead_code))]
pub(crate) enum Action {
    Speed(Option<String>),
    FastForward(FastForwardAction),
}
impl Action {
    pub(crate) fn changes_machine(&self) -> bool {
        matches!(
            self,
            Self::Speed(Some(_)) | Self::FastForward(FastForwardAction::Start(_))
        )
    }
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(feature = "mcp"), allow(dead_code))]
pub(crate) enum FastForwardAction {
    Get,
    Start(Duration),
    Stop,
}

/// Only acceleration started by MCP is owned here. Physical Alt+F12 can
/// explicitly take it over; expiry must then leave the human's pacing alone.
#[derive(Default)]
pub(crate) struct FastForward {
    lease: Option<(Instant, u32)>,
}
impl FastForward {
    pub(crate) fn handoff(&mut self) {
        self.lease = None;
    }

    pub(crate) fn poll(&mut self, cpu: &mut Cpu, pacer: &mut Pacer, now: Instant, reset: bool) {
        if self.lease.is_some_and(|(deadline, generation)| {
            reset || now >= deadline || generation != cpu.bus.power_ons
        }) {
            self.lease = None;
            pacer.set_fast_forward(false, &cpu.bus.clock, now);
            cpu.bus.mixer.fast_forward = false;
        }
    }

    pub(crate) fn request(
        &mut self,
        action: FastForwardAction,
        cpu: &mut Cpu,
        pacer: &mut Pacer,
        now: Instant,
        blocked: bool,
        deterministic: bool,
    ) -> Result<Value, String> {
        self.poll(cpu, pacer, now, blocked);
        match action {
            FastForwardAction::Get => {}
            FastForwardAction::Start(duration) => {
                if duration.is_zero() || duration > Duration::from_millis(MAX_FAST_FORWARD_MS) {
                    return Err("fast-forward duration must be 1..5000 milliseconds".into());
                }
                if deterministic || blocked {
                    return Err("fast-forward requires a running real-time guest outside settings or rewind".into());
                }
                if pacer.fast_forward() {
                    return Err("fast-forward is already active; inspect it before starting another interval".into());
                }
                self.lease = Some((now + duration, cpu.bus.power_ons));
                pacer.set_fast_forward(true, &cpu.bus.clock, now);
                cpu.bus.mixer.fast_forward = true;
            }
            FastForwardAction::Stop => self.poll(cpu, pacer, now, true),
        }
        Ok(json!({
            "enabled": pacer.fast_forward(),
            "agent_active": self.lease.is_some(),
            "remaining_ms": self.lease.map_or(0, |(deadline, _)| deadline.saturating_duration_since(now).as_millis()),
            "multiplier": Pacer::fast_forward_multiplier(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timer::CpuSpeed;

    #[test]
    fn bounded_lease_expiry_handoff_and_generation_preserve_foreign_pacing() {
        let mut cpu = Cpu::new(std::env::temp_dir());
        let start = Instant::now();
        let mut pacer = Pacer::new(CpuSpeed::Fixed(3000), None, start);
        let mut lease = FastForward::default();
        let duration = Duration::from_millis(100);
        let state = lease
            .request(
                FastForwardAction::Start(duration),
                &mut cpu,
                &mut pacer,
                start,
                false,
                false,
            )
            .unwrap();
        assert_eq!(state["agent_active"], true);
        assert_eq!(state["remaining_ms"], 100);
        assert!(cpu.bus.mixer.fast_forward && pacer.fast_forward());
        assert!(
            lease
                .request(
                    FastForwardAction::Start(duration),
                    &mut cpu,
                    &mut pacer,
                    start,
                    false,
                    false
                )
                .is_err()
        );
        lease.poll(&mut cpu, &mut pacer, start + duration, false);
        assert!(!pacer.fast_forward() && !cpu.bus.mixer.fast_forward);
        assert!(lease.lease.is_none());

        lease
            .request(
                FastForwardAction::Start(duration),
                &mut cpu,
                &mut pacer,
                start,
                false,
                false,
            )
            .unwrap();
        lease.handoff();
        lease.poll(&mut cpu, &mut pacer, start + duration, true);
        lease
            .request(
                FastForwardAction::Stop,
                &mut cpu,
                &mut pacer,
                start + duration,
                false,
                false,
            )
            .unwrap();
        assert!(
            pacer.fast_forward() && cpu.bus.mixer.fast_forward,
            "manual takeover survives agent stop and expiry"
        );
        assert!(
            lease
                .request(
                    FastForwardAction::Start(duration),
                    &mut cpu,
                    &mut pacer,
                    start,
                    false,
                    false
                )
                .is_err(),
            "foreign acceleration cannot be acquired"
        );
        pacer.set_fast_forward(false, &cpu.bus.clock, start);
        cpu.bus.mixer.fast_forward = false;

        for (blocked, deterministic, duration) in [
            (true, false, duration),
            (false, true, duration),
            (false, false, Duration::ZERO),
            (false, false, Duration::from_millis(MAX_FAST_FORWARD_MS + 1)),
        ] {
            assert!(
                lease
                    .request(
                        FastForwardAction::Start(duration),
                        &mut cpu,
                        &mut pacer,
                        start,
                        blocked,
                        deterministic
                    )
                    .is_err()
            );
            assert!(!pacer.fast_forward() && lease.lease.is_none());
        }
        lease
            .request(
                FastForwardAction::Start(duration),
                &mut cpu,
                &mut pacer,
                start,
                false,
                false,
            )
            .unwrap();
        cpu.bus.power_ons += 1;
        lease.poll(&mut cpu, &mut pacer, start, false);
        assert!(!pacer.fast_forward() && !cpu.bus.mixer.fast_forward);
        lease
            .request(
                FastForwardAction::Start(duration),
                &mut cpu,
                &mut pacer,
                start,
                false,
                false,
            )
            .unwrap();
        lease.poll(&mut cpu, &mut pacer, start, true);
        assert!(!pacer.fast_forward() && !cpu.bus.mixer.fast_forward);
    }
}
