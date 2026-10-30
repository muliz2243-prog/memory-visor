pub const DEFAULT_STEP_MS: u32 = 700;

/// A window that was minimised can report a huge delta. A tick never advances
/// more than this, so the animation stays readable.
pub const MAX_TICK_MS: u32 = 250;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tick {
    pub delta_ms: u32,
}

impl Tick {
    pub fn from_seconds(seconds: f32) -> Self {
        let millis = seconds * 1000.0;
        Tick {
            delta_ms: match millis.is_finite() && millis > 0.0 {
                true => millis.min(MAX_TICK_MS as f32) as u32,
                false => 0,
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Player {
    /// Number of events already applied.
    pub index: usize,
    pub step_ms: u32,
    pub accumulated_ms: u32,
    pub paused: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayerPlan {
    /// Apply the event at `index`, then store `accumulated_ms`.
    Advance {
        index: usize,
        accumulated_ms: u32,
    },
    Wait {
        accumulated_ms: u32,
    },
    Finished,
}

impl Player {
    pub fn new() -> Self {
        Player {
            index: 0,
            step_ms: DEFAULT_STEP_MS,
            accumulated_ms: 0,
            paused: true,
        }
    }

    pub fn plan(&self, tick: Tick, event_count: usize) -> PlayerPlan {
        match self.paused {
            true => PlayerPlan::Wait {
                accumulated_ms: self.accumulated_ms,
            },
            false => self.running_plan(tick, event_count),
        }
    }

    fn running_plan(&self, tick: Tick, event_count: usize) -> PlayerPlan {
        let elapsed = self.accumulated_ms.saturating_add(tick.delta_ms);
        let due = elapsed >= self.step_ms.max(1);
        match (self.index >= event_count, due) {
            (true, _) => PlayerPlan::Finished,
            (false, true) => PlayerPlan::Advance {
                index: self.index + 1,
                accumulated_ms: elapsed - self.step_ms.max(1),
            },
            (false, false) => PlayerPlan::Wait {
                accumulated_ms: elapsed,
            },
        }
    }

    pub fn apply(&self, plan: PlayerPlan) -> Player {
        match plan {
            PlayerPlan::Advance {
                index,
                accumulated_ms,
            } => Player {
                index,
                accumulated_ms,
                ..*self
            },
            PlayerPlan::Wait { accumulated_ms } => Player {
                accumulated_ms,
                ..*self
            },
            PlayerPlan::Finished => *self,
        }
    }

    pub fn paused(&self) -> Player {
        Player {
            paused: true,
            ..*self
        }
    }

    pub fn running(&self) -> Player {
        Player {
            paused: false,
            ..*self
        }
    }

    pub fn at(&self, index: usize) -> Player {
        Player {
            index,
            accumulated_ms: 0,
            ..*self
        }
    }
}

impl Default for Player {
    fn default() -> Self {
        Player::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_paused_with_nothing_applied() {
        let player = Player::new();
        assert_eq!(player.index, 0);
        assert!(player.paused);
        assert_eq!(
            player.plan(Tick { delta_ms: 1000 }, 3),
            PlayerPlan::Wait { accumulated_ms: 0 }
        );
    }

    #[test]
    fn advances_when_the_step_elapsed() {
        let player = Player::new().running();
        let plan = player.plan(Tick { delta_ms: 700 }, 3);
        assert_eq!(
            plan,
            PlayerPlan::Advance {
                index: 1,
                accumulated_ms: 0
            }
        );
        assert_eq!(player.apply(plan).index, 1);
    }

    #[test]
    fn accumulates_partial_ticks() {
        let player = Player::new().running();
        let plan = player.plan(Tick { delta_ms: 250 }, 3);
        assert_eq!(
            plan,
            PlayerPlan::Wait {
                accumulated_ms: 250
            }
        );
        let second = player.apply(plan).plan(Tick { delta_ms: 250 }, 3);
        assert_eq!(
            second,
            PlayerPlan::Wait {
                accumulated_ms: 500
            }
        );
        let third = player.apply(second).plan(Tick { delta_ms: 250 }, 3);
        assert_eq!(
            third,
            PlayerPlan::Advance {
                index: 1,
                accumulated_ms: 50
            }
        );
    }

    #[test]
    fn stops_at_the_end_of_the_trace() {
        let player = Player::new().running().at(3);
        assert_eq!(player.plan(Tick { delta_ms: 700 }, 3), PlayerPlan::Finished);
    }

    #[test]
    fn a_clamped_tick_never_jumps_more_than_one_step() {
        let player = Player::new().running();
        let tick = Tick::from_seconds(12.0);
        assert_eq!(tick.delta_ms, MAX_TICK_MS);
        // 250 ms is below the 700 ms step, so the cursor waits instead of
        // skipping frames the user never saw.
        assert_eq!(
            player.plan(tick, 10),
            PlayerPlan::Wait {
                accumulated_ms: MAX_TICK_MS
            }
        );
        let settled = player.apply(player.plan(tick, 10));
        assert_eq!(settled.index, 0);
    }

    #[test]
    fn a_broken_delta_is_treated_as_zero() {
        assert_eq!(Tick::from_seconds(f32::NAN).delta_ms, 0);
        assert_eq!(Tick::from_seconds(-1.0).delta_ms, 0);
        assert_eq!(Tick::from_seconds(0.05).delta_ms, 50);
    }
}
